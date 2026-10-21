package com.joymanga.app

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.*
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.*
import android.provider.*
import android.webkit.*
import android.view.inputmethod.InputMethodManager
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.view.*
import androidx.activity.OnBackPressedCallback
import android.view.KeyEvent
import android.view.WindowManager
import org.json.*
import java.io.*
import java.util.UUID
import java.util.concurrent.Executors

class MainActivity : TauriActivity() {
    private var appWebView: WebView? = null
    private var pendingPicker: Picker? = null
    private val worker = Executors.newSingleThreadExecutor()
    // 更新包下载单独占一个线程：大文件下载耗时可能很久，不能挡住导入/导出的暂存清理。
    private val updateWorker = Executors.newSingleThreadExecutor()
    @Volatile private var updateDownloading = false
    private var visible = false
    private var activeDownloads = 0
    private val handler = Handler(Looper.getMainLooper())
    private var serviceUpdateScheduled = false
    private var pendingViewUri: Uri? = null
    private var downloadSpeed = ""
    private var downloadPagesCurrent = 0
    private var downloadPagesTotal = 0
    internal val webView: WebView? get() = appWebView
    private var volumeKeyPaging = false

    private data class Picker(val id: String, val mode: String, val exportPath: String? = null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        WindowCompat.setDecorFitsSystemWindows(window, true)
        // 界面恒为浅色：系统深色模式下窗口背景会变深，与固定浅色的 WebView 冲突，
        // 启动瞬间还会闪一下。显式锁成浅色并让状态栏图标对应深色文字。
        AppCompatDelegate.setDefaultNightMode(AppCompatDelegate.MODE_NIGHT_NO)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() { dispatchBack() }
        })
        worker.execute {
            // 清理上次异常退出遗留的暂存；updates 目录里是下载好的 APK，保留给安装器读取
            try { importRoot().listFiles()?.forEach { it.deleteRecursively() } } catch (_: Exception) {}
            try { openRoot().listFiles()?.filter { it.lastModified() < System.currentTimeMillis() - 24 * 60 * 60 * 1000 }?.forEach { it.deleteRecursively() } } catch (_: Exception) {}
            try { File(cacheDir, "android-imports").deleteRecursively() } catch (_: Exception) {}
        }
        // 冷启动时由「用 JoyManga 打开 CBZ」拉起：WebView 就绪后会自动导入
        handleViewIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleViewIntent(intent)
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        appWebView = webView
        webView.addJavascriptInterface(AndroidBridge(), "JmAndroid")
        registerNetworkCallback()
        pendingViewUri?.let { uri ->
            pendingViewUri = null
            dispatchViewUri(uri)
        }
    }

    /**
     * 「用 JoyManga 打开 CBZ」：把外部文件复制到导入暂存区，再通知前端导入。
     * 前端可能还没挂载监听器，所以冷启动时先存起来，等 WebView 创建后再发。
     */
    private fun handleViewIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val uri = intent.data ?: return
        intent.action = null
        pendingViewUri = uri
        appWebView?.let {
            pendingViewUri = null
            dispatchViewUri(uri)
        }
    }

    private fun dispatchViewUri(uri: Uri) {
        val requestId = UUID.randomUUID().toString()
        worker.execute {
            try {
                val name = safeName(documentName(uri))
                require(name.endsWith(".cbz", ignoreCase = true)) { "只支持打开 CBZ 文件" }
                // 外部打开的漫画可能很大，cacheDir 在存储紧张时会被系统清理，
                // 放到 filesDir 下的专门目录，导入完成后由前端调 discardImports 删除。
                val staging = File(openRoot(), requestId).apply { mkdirs() }
                val target = File(staging, name)
                copyUri(uri, target, 2L * 1024 * 1024 * 1024)
                runOnUiThread {
                    appWebView?.evaluateJavascript(
                        "window.dispatchEvent(new CustomEvent('jm-open-file', {detail:{id:${JSONObject.quote(requestId)},path:${JSONObject.quote(target.absolutePath)}}}))",
                        null,
                    )
                }
            } catch (error: Exception) {
                File(openRoot(), requestId).deleteRecursively()
                runOnUiThread { android.widget.Toast.makeText(this, "打开文件失败：${error.message ?: "未知错误"}", android.widget.Toast.LENGTH_LONG).show() }
            }
        }
    }

    /**
     * 网络切换（Wi-Fi <-> 移动数据）时把"是否计费"推给前端，供「仅 Wi-Fi 下载」判断。
     *
     * 必须监听**默认网络**：按 INTERNET 能力注册回调时，Wi-Fi 与移动数据会各自
     * 触发一次，最后到达的那个说了算——Wi-Fi + 蜂窝并存（或开 VPN）时会把
     * 不计费的 Wi-Fi 误判成计费网络，导致「仅 Wi-Fi 下载」无故暂停。
     */
    private fun registerNetworkCallback() {
        try {
            val manager = getSystemService(ConnectivityManager::class.java)
            manager.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                    pushMetered(!capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED))
                }
                override fun onLost(network: Network) { pushMetered(manager.isActiveNetworkMetered) }
            })
            pushMetered(manager.isActiveNetworkMetered)
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "读取网络计费状态失败", error)
        }
    }

    private fun pushMetered(metered: Boolean) {
        if (!visible) return
        runOnUiThread {
            appWebView?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('jm-network-metered', {detail:$metered}))", null)
        }
    }

    override fun onStart() {
        super.onStart()
        visible = true
        AppState.activity = this
        updateDownloadService()
        pushMetered(getSystemService(ConnectivityManager::class.java).isActiveNetworkMetered)
    }
    override fun onStop() {
        visible = false
        if (AppState.activity === this) AppState.activity = null
        // 不要在这里清 KEEP_SCREEN_ON：窗口标志只在窗口可见时生效，不可见时本来就不会
        // 阻止息屏；清掉后前端不会再设置（watcher 只在配置/阅读状态变化时触发），
        // 用户切出去一次再回来，本次阅读就一直会息屏。
        super.onStop()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) { dispatchBack(); return true }
        if (volumeKeyPaging && (keyCode == KeyEvent.KEYCODE_VOLUME_UP || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN)) {
            if (event?.repeatCount == 0) {
                val direction = if (keyCode == KeyEvent.KEYCODE_VOLUME_UP) "prev" else "next"
                appWebView?.evaluateJavascript(
                    "window.dispatchEvent(new CustomEvent('jm-volume-page', {detail:'$direction'}))", null)
            }
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    private fun dispatchBack() {
        val view = appWebView ?: run { moveTaskToBack(true); return }
        if (ViewCompat.getRootWindowInsets(view)?.isVisible(WindowInsetsCompat.Type.ime()) == true) {
            (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).hideSoftInputFromWindow(view.windowToken, 0)
            return
        }
        view.evaluateJavascript("window.dispatchEvent(new Event('jm-android-back'))", null)
    }

    private fun result(id: String, paths: List<String>? = null, error: String? = null, cancelled: Boolean = false, exported: Int? = null) {
        val payload = JSONObject().put("requestId", id).put("cancelled", cancelled)
        if (paths != null) payload.put("paths", JSONArray(paths))
        if (error != null) payload.put("error", error)
        if (exported != null) payload.put("exported", exported)
        runOnUiThread { appWebView?.evaluateJavascript("window.dispatchEvent(new CustomEvent('jm-mobile-result', {detail:$payload}))", null) }
    }

    private fun libraryRoot() = File(getExternalFilesDir(null) ?: filesDir, "library").apply { mkdirs() }
    // 导入暂存放 filesDir 而不是 cacheDir：大文件复制耗时较长，期间系统可能清理缓存目录，
    // 导致复制到一半的文件被删。导入结束后由前端 discardImports 清理。
    private fun importRoot() = File(filesDir, "android-imports").apply { mkdirs() }
    private fun openRoot() = File(filesDir, "android-opens").apply { mkdirs() }
    private fun validId(id: String): Boolean = try { UUID.fromString(id); true } catch (_: Exception) { false }

    private fun beginPicker(picker: Picker) {
        if (pendingPicker != null) { result(picker.id, error = "已有文件选择窗口，请先完成或取消"); return }
        pendingPicker = picker
        val tree = picker.mode == "directory" || picker.mode == "export-directory"
        val intent = if (tree) Intent(Intent.ACTION_OPEN_DOCUMENT_TREE) else Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = if (picker.mode == "backup") "application/json" else "*/*"
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, picker.mode == "cbz")
            if (picker.mode == "cbz") putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("application/zip", "application/x-cbz", "application/vnd.comicbook+zip", "application/octet-stream"))
        }
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        if (picker.mode == "export-directory") intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        try { startActivityForResult(intent, PICK_DOCUMENTS) }
        catch (error: Exception) { pendingPicker = null; result(picker.id, error = error.message ?: "无法打开系统文件选择器") }
    }

    /** 运行时权限结果回传：API 29 以下的存储授权没有可见性变化事件，前端靠这个事件完成引导。 */
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != 102) return
        val granted = grantResults.isNotEmpty() && grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        runOnUiThread {
            appWebView?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('jm-permission-result', {detail:{request:'storage',granted:$granted}}))", null)
        }
    }

    @Deprecated("Compatible document picker result")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != PICK_DOCUMENTS) return
        val picker = pendingPicker ?: return
        pendingPicker = null
        if (resultCode != RESULT_OK || data == null) { result(picker.id, cancelled = true); return }
        val uris = buildList {
            data.clipData?.let { clip -> for (index in 0 until clip.itemCount) add(clip.getItemAt(index).uri) }
            if (isEmpty()) data.data?.let { add(it) }
        }
        if (uris.isEmpty()) { result(picker.id, cancelled = true); return }
        worker.execute {
            try {
                if (picker.mode == "export-directory") {
                    val uri = uris.first()
                    val count = publishToTree(File(picker.exportPath!!), uri)
                    result(picker.id, exported = count)
                    return@execute
                }
                require(uris.size <= 100) { "单次最多选择 100 个文件" }
                val staging = File(importRoot(), picker.id).apply { mkdirs() }
                val paths = if (picker.mode == "directory") {
                    val source = uris.first()
                    val root = DocumentsContract.buildDocumentUriUsingTree(source, DocumentsContract.getTreeDocumentId(source))
                    val target = File(staging, safeName(documentName(root))).apply { mkdirs() }
                    val counter = intArrayOf(0)
                    copyTree(source, root, target, 0, counter)
                    listOf(target.absolutePath)
                } else uris.map { uri ->
                    val name = safeName(documentName(uri))
                    val expected = if (picker.mode == "backup") ".json" else ".cbz"
                    require(name.endsWith(expected, ignoreCase = true)) { "请选择 $expected 文件" }
                    val target = uniqueFile(staging, name)
                    copyUri(uri, target, if (picker.mode == "backup") 8 * 1024 * 1024L else 2 * 1024 * 1024 * 1024L)
                    target.absolutePath
                }
                result(picker.id, paths = paths)
            } catch (error: Exception) {
                File(importRoot(), picker.id).deleteRecursively()
                result(picker.id, error = error.message ?: "读取所选文件失败")
            }
        }
    }

    private fun documentName(uri: Uri): String = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
        if (it.moveToFirst()) it.getString(0) ?: "file" else "file"
    } ?: "file"

    private fun safeName(name: String): String {
        val clean = name.replace(Regex("""[\\/:*?"<>|]"""), "_").trim().ifBlank { "file" }
        if (clean == "." || clean == "..") return "file"
        val extension = clean.substringAfterLast('.', "").lowercase()
        val suffix = if (extension in listOf("cbz", "json", "jpg", "jpeg", "png", "webp")) ".$extension" else ""
        val stem = if (suffix.isEmpty()) clean else clean.dropLast(extension.length + 1)
        val result = StringBuilder()
        var bytes = 0
        var index = 0
        val limit = 180 - suffix.toByteArray(Charsets.UTF_8).size
        while (index < stem.length) {
            val point = stem.codePointAt(index)
            val piece = String(Character.toChars(point))
            val count = piece.toByteArray(Charsets.UTF_8).size
            if (bytes + count > limit) break
            result.append(piece); bytes += count; index += Character.charCount(point)
        }
        return result.toString().ifBlank { "file" } + suffix
    }
    private fun uniqueFile(parent: File, name: String): File {
        var file = File(parent, name)
        if (file.exists()) file = File(parent, UUID.randomUUID().toString().take(8) + "-" + name)
        return file
    }

    private fun copyUri(uri: Uri, target: File, limit: Long) {
        val freeSpace = target.parentFile?.usableSpace ?: Long.MAX_VALUE
        require(freeSpace > 50 * 1024 * 1024L) { "设备存储空间不足" }
        val part = File(target.parentFile, ".part-" + UUID.randomUUID())
        try {
            contentResolver.openInputStream(uri).use { input ->
                require(input != null) { "文件提供器无法读取所选文件" }
                FileOutputStream(part).use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val count = input.read(buffer); if (count < 0) break
                        total += count; require(total <= limit) { "所选文件超过大小上限" }
                        output.write(buffer, 0, count)
                    }
                    output.fd.sync()
                }
            }
            require(part.renameTo(target)) { "无法提交导入临时文件" }
        } finally { part.delete() }
    }

    private fun copyTree(tree: Uri, document: Uri, target: File, depth: Int, counter: IntArray) {
        require(depth <= 10) { "目录层级过深" }
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getDocumentId(document))
        contentResolver.query(children, arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_MIME_TYPE), null, null, null)?.use { cursor ->
            while (cursor.moveToNext()) {
                require(++counter[0] <= 10000) { "目录中文件过多" }
                val uri = DocumentsContract.buildDocumentUriUsingTree(tree, cursor.getString(0))
                val name = safeName(cursor.getString(1) ?: "file")
                val type = cursor.getString(2)
                if (type == DocumentsContract.Document.MIME_TYPE_DIR) {
                    val directory = uniqueFile(target, name).apply { mkdirs() }
                    copyTree(tree, uri, directory, depth + 1, counter)
                } else if (name.endsWith(".cbz", true) || listOf(".jpg", ".jpeg", ".png", ".webp").any { name.endsWith(it, true) }) {
                    copyUri(uri, uniqueFile(target, name), if (name.endsWith(".cbz", true)) 2 * 1024 * 1024 * 1024L else 32 * 1024 * 1024L)
                }
            }
        } ?: throw IOException("文件提供器无法读取目录")
    }

    private fun exportFiles(path: File): List<File> {
        val canonical = path.canonicalFile
        // 书库可能是应用专用目录、公共 Download/JoyManga，或用户通过目录选择器挑的
        // 其他公共目录；三者都允许导出（无存储权限时公共路径的读取会自然失败）。
        // 只发布 cbz/json/txt，避免把目录里的其他文件一起复制出去。
        val allowed = listOfNotNull(filesDir, getExternalFilesDir(null), Environment.getExternalStorageDirectory()).map { it.canonicalFile }
        require(allowed.any { canonical.path == it.path || canonical.path.startsWith(it.path + File.separator) }) { "请先将文件导出到应用专用目录或公共存储" }
        val files = if (canonical.isFile) listOf(canonical) else canonical.walkTopDown().filter { it.isFile }.take(1001).toList()
        require(files.size <= 1000) { "单次最多导出 1000 个文件" }
        return files.filter { file -> listOf("cbz", "json", "txt").contains(file.extension.lowercase()) }
    }

    private fun publicLibraryRoot() =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "JoyManga")

    private fun mime(file: File) = when (file.extension.lowercase()) {
        "json" -> "application/json"
        "txt" -> "text/plain"
        else -> "application/zip"
    }

    private fun publishDownloads(path: File): Int {
        val files = exportFiles(path)
        require(files.isNotEmpty()) { "没有可导出的 CBZ/备份文件" }
        for (file in files) {
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, file.name)
                put(MediaStore.MediaColumns.MIME_TYPE, mime(file))
                put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/JoyManga")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val uri = contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw IOException("创建公共下载文件失败")
            try {
                contentResolver.openFileDescriptor(uri, "w").use { descriptor ->
                    require(descriptor != null) { "无法写入公共文件" }
                    FileOutputStream(descriptor.fileDescriptor).use { output -> file.inputStream().use { it.copyTo(output) }; output.fd.sync() }
                }
                require(contentResolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null) == 1) { "发布公共文件失败" }
            } catch (error: Exception) { contentResolver.delete(uri, null, null); throw error }
        }
        return files.size
    }

    private fun publishToTree(path: File, tree: Uri): Int {
        val files = exportFiles(path)
        require(files.isNotEmpty()) { "没有可导出的文件" }
        val parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        for (file in files) {
            val staged = DocumentsContract.createDocument(contentResolver, parent, mime(file), ".part-" + UUID.randomUUID())
                ?: throw IOException("文件提供器无法创建文件")
            try {
                contentResolver.openOutputStream(staged, "w").use { output -> require(output != null); file.inputStream().use { it.copyTo(output) } }
                require(DocumentsContract.renameDocument(contentResolver, staged, file.name) != null) { "此位置不支持完整文件提交，请换一个系统文件夹" }
            } catch (error: Exception) { DocumentsContract.deleteDocument(contentResolver, staged); throw error }
        }
        return files.size
    }

    private fun updateDownloadService() {
        serviceUpdateScheduled = false
        if (activeDownloads <= 0) { stopService(Intent(this, DownloadService::class.java)); return }
        val intent = Intent(this, DownloadService::class.java)
            .putExtra("active", activeDownloads)
            .putExtra("speed", downloadSpeed)
            .putExtra("current", downloadPagesCurrent)
            .putExtra("total", downloadPagesTotal)
        try {
            if (visible) {
                if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent) else startService(intent)
            } else {
                // 已运行服务可接收更新；不能从后台创建新的前台服务。
                getSystemService(NotificationManager::class.java).notify(DownloadService.ID,
                    (if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, DownloadService.CHANNEL) else Notification.Builder(this))
                        .setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle("正在下载 $activeDownloads 个章节")
                        .setContentText(downloadSpeed).setOngoing(true).setOnlyAlertOnce(true)
                        .setContentIntent(android.app.PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE)).build())
            }
        } catch (error: Exception) { android.util.Log.w("JoyManga", "下载前台服务启动失败", error) }
    }

    /**
     * 合并高频更新：每张图片下载完成前端都会推一次状态，多章节并发时每秒可达十几次。
     * 每次都重建通知会被系统限流（通知被丢弃或延迟），所以这里按 750ms 合并，
     * 最后一次调度总会用最新值执行。
     */
    private fun scheduleDownloadServiceUpdate() {
        if (serviceUpdateScheduled) return
        serviceUpdateScheduled = true
        handler.postDelayed({ updateDownloadService() }, 750)
    }

    /** 全部任务结束时发一条可自动消失的完成通知（前台通知会随服务停止一起消失）。 */
    private fun notifyDownloadsFinished() {
        try {
            DownloadService.ensureChannel(this)
            val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
            val pending = android.app.PendingIntent.getActivity(this, 0, open, android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE)
            val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, DownloadService.CHANNEL) else Notification.Builder(this)
            getSystemService(NotificationManager::class.java).notify(
                DownloadService.DONE_ID,
                builder.setSmallIcon(android.R.drawable.stat_sys_download_done)
                    .setContentTitle("下载完成").setContentText("点击查看下载与传输")
                    .setContentIntent(pending).setAutoCancel(true).setOnlyAlertOnce(true).build(),
            )
        } catch (error: Exception) { android.util.Log.w("JoyManga", "发送下载完成通知失败", error) }
    }

    inner class AndroidBridge {
        @JavascriptInterface fun libraryPath(): String = libraryRoot().absolutePath
        /** 公共下载目录下的默认书库：文件管理器直接可见，卸载应用不随之删除。 */
        @JavascriptInterface fun downloadsPath(): String = publicLibraryRoot().absolutePath
        /** 是否已具备公共存储写权限：API 30+ 看「所有文件访问」，API 29 无可用方案，更老版本看运行时权限。 */
        @JavascriptInterface fun hasAllFilesAccess(): Boolean =
            when {
                // isExternalStorageManager 是 API 30 新增，API 29 调用会抛 NoSuchMethodError
                Build.VERSION.SDK_INT >= 30 -> Environment.isExternalStorageManager()
                Build.VERSION.SDK_INT < 29 -> checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                // Android 10 既没有 MANAGE_EXTERNAL_STORAGE，WRITE_EXTERNAL_STORAGE 又被 maxSdkVersion=28 屏蔽
                else -> false
            }
        /** 当前系统是否支持把书库放到公共下载目录（Android 10 上不可用）。 */
        @JavascriptInterface fun supportsPublicStorage(): Boolean =
            Build.VERSION.SDK_INT >= 30 || Build.VERSION.SDK_INT < 29
        /** 跳系统页授予「所有文件访问」；API 29 以下弹运行时权限对话框（结果经 jm-permission-result 回传）。 */
        @JavascriptInterface fun requestAllFilesAccess() {
            runOnUiThread {
                when {
                    Build.VERSION.SDK_INT >= 30 -> {
                        try { startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:$packageName"))) }
                        catch (_: Exception) { try { startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)) } catch (_: Exception) {} }
                    }
                    Build.VERSION.SDK_INT < 29 -> requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), 102)
                    // API 29：没有任何可授予的公共存储权限，前端已隐藏该选项
                }
            }
        }
        /** 公共下载目录是否可写（写探测失败时前端退回应用专用目录，避免初始化失败锁死）。 */
        @JavascriptInterface fun canWritePublicDownloads(): Boolean = try {
            val dir = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "JoyManga")
            if (!dir.exists() && !dir.mkdirs()) false
            else {
                val probe = File(dir, ".write_test_${UUID.randomUUID()}")
                probe.writeText("test")
                probe.delete()
                true
            }
        } catch (_: Exception) { false }
        @JavascriptInterface fun exitApp() { runOnUiThread { moveTaskToBack(true) } }
        /**
         * 应用内下载 APK 并拉起系统安装器。
         *
         * 浏览器下载再返回应用很折腾；这里下到私有缓存目录，经 FileProvider 授权后
         * 交给 PackageInstaller（用户仍需在系统弹窗确认）。进度通过
         * `jm-update-progress` 事件回传。
         */
        @JavascriptInterface fun downloadAndInstallApk(url: String, fileName: String) {
            if (!url.startsWith("https://")) { emitUpdateEvent("error", message = "更新地址无效"); return }
            // 先确认能装再下载：否则几十 MB 的安装包下完才发现没有权限，白费流量
            if (Build.VERSION.SDK_INT >= 26 && !packageManager.canRequestPackageInstalls()) {
                emitUpdateEvent("permission", message = "请在设置中允许 JoyManga 安装未知应用，然后重新点击更新")
                runOnUiThread {
                    try { startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))) }
                    catch (error: Exception) { android.util.Log.w("JoyManga", "无法打开安装未知应用设置页", error) }
                }
                return
            }
            if (updateDownloading) { emitUpdateEvent("error", message = "安装包正在下载中"); return }
            updateDownloading = true
            updateWorker.execute {
                try {
                    val safeName = safeName(fileName).ifBlank { "update.apk" }.let {
                        if (it.endsWith(".apk", ignoreCase = true)) it else "$it.apk"
                    }
                    val dir = File(cacheDir, "updates").apply { mkdirs() }
                    // 只清理旧安装包：系统安装器可能还在读取上一次下载的文件
                    dir.listFiles()?.filter { it.lastModified() < System.currentTimeMillis() - 60 * 60 * 1000 }?.forEach { it.delete() }
                    val target = File(dir, safeName)
                    val part = File(dir, ".part-$safeName")
                    val connection = java.net.URL(url).openConnection() as java.net.HttpURLConnection
                    try {
                        connection.connectTimeout = 15_000
                        connection.readTimeout = 30_000
                        connection.instanceFollowRedirects = true
                        // GitHub 对缺少 UA 的请求可能直接返回 403
                        connection.setRequestProperty("User-Agent", "JoyManga-Android")
                        connection.connect()
                        require(connection.responseCode in 200..299) { "下载失败（HTTP ${connection.responseCode}）" }
                        val total = connection.contentLengthLong
                        connection.inputStream.use { input ->
                            FileOutputStream(part).use { output ->
                                val buffer = ByteArray(128 * 1024)
                                var copied = 0L
                                var lastReport = 0L
                                val step = (total / 100).coerceAtLeast(64 * 1024)
                                while (true) {
                                    val count = input.read(buffer)
                                    if (count < 0) break
                                    output.write(buffer, 0, count)
                                    copied += count
                                    if (total > 0 && copied - lastReport >= step) {
                                        lastReport = copied
                                        emitUpdateEvent("progress", percent = (copied * 100 / total).toInt().coerceIn(0, 99))
                                    }
                                }
                                output.fd.sync()
                            }
                        }
                    } finally {
                        connection.disconnect()
                    }
                    require(part.length() > 0) { "下载的安装包为空" }
                    require(part.renameTo(target)) { "无法保存安装包" }
                    emitUpdateEvent("done", percent = 100)
                    runOnUiThread { installApk(target) }
                } catch (error: Exception) {
                    emitUpdateEvent("error", message = error.message ?: "下载更新失败")
                } finally {
                    updateDownloading = false
                }
            }
        }
        @JavascriptInterface fun pickDocuments(id: String, mode: String) {
            if (!validId(id)) return
            if (mode !in listOf("cbz", "backup", "directory")) { result(id, error = "文件选择模式无效"); return }
            runOnUiThread { beginPicker(Picker(id, mode)) }
        }
        @JavascriptInterface fun discardImports(id: String) {
            if (validId(id)) worker.execute {
                File(importRoot(), id).deleteRecursively()
                File(openRoot(), id).deleteRecursively()
            }
        }
        @JavascriptInterface fun exportDocuments(id: String, path: String) {
            if (!validId(id)) return
            if (Build.VERSION.SDK_INT >= 29) worker.execute {
                try { result(id, exported = publishDownloads(File(path))) }
                catch (error: Exception) { result(id, error = error.message ?: "导出失败") }
            } else runOnUiThread { beginPicker(Picker(id, "export-directory", path)) }
        }
        @JavascriptInterface fun downloadStatus(active: Int, speed: String, current: Int, total: Int) {
            runOnUiThread {
                val wasActive = activeDownloads > 0
                activeDownloads = active.coerceAtLeast(0); downloadSpeed = speed.take(80)
                downloadPagesCurrent = current.coerceAtLeast(0)
                downloadPagesTotal = total.coerceAtLeast(0)
                // 用户第一次拒绝后不再骚扰，但每次升级/新会话仍允许再试一次：
                // 记的是"已询问过的应用版本"，而不是永久开关。
                val prefs = getSharedPreferences("joymanga_permissions", MODE_PRIVATE)
                val version = appVersionName()
                val askedVersion = prefs.getString("notifications_prompted_version", null)
                if (activeDownloads > 0 && Build.VERSION.SDK_INT >= 33 &&
                    checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
                    askedVersion != version) {
                    prefs.edit().putString("notifications_prompted_version", version).apply()
                    requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 101)
                }
                scheduleDownloadServiceUpdate()
                // 服务停止后前台通知随之消失，这里补一条完成通知，避免用户不知道已经下完。
                if (wasActive && activeDownloads == 0) notifyDownloadsFinished()
            }
        }
        /** 阅读时保持屏幕常亮；关闭后恢复系统默认息屏策略。 */
        @JavascriptInterface fun setKeepScreenOn(enabled: Boolean) {
            runOnUiThread {
                if (enabled) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        /** 音量键翻页：由前端根据设置开启，原生侧只在开启时接管按键。 */
        @JavascriptInterface fun setVolumeKeyPaging(enabled: Boolean) {
            runOnUiThread { volumeKeyPaging = enabled }
        }
    }
    /** 把下载好的 APK 交给系统安装器；未授予「安装未知应用」时先跳到对应设置页。 */
    private fun installApk(apk: File) {
        if (Build.VERSION.SDK_INT >= 26 && !packageManager.canRequestPackageInstalls()) {
            emitUpdateEvent("permission", message = "请在设置中允许 JoyManga 安装未知应用，然后重新点击更新")
            try {
                startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName")))
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "无法打开安装未知应用设置页", error)
            }
            return
        }
        try {
            val uri = androidx.core.content.FileProvider.getUriForFile(this, "$packageName.fileprovider", apk)
            startActivity(Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "application/vnd.android.package-archive")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            })
            emitUpdateEvent("installer", message = "请在系统安装器中确认安装")
        } catch (error: Exception) {
            emitUpdateEvent("error", message = "无法拉起系统安装器：${error.message ?: "未知错误"}")
        }
    }

    /** 更新流程进度/结果回传前端；事件名与 `src/utils/androidStorage.ts` 的订阅保持一致。 */
    private fun emitUpdateEvent(state: String, percent: Int = -1, message: String = "") {
        val payload = JSONObject().put("state", state)
        if (percent >= 0) payload.put("percent", percent)
        if (message.isNotEmpty()) payload.put("message", message)
        runOnUiThread {
            appWebView?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('jm-update-progress', {detail:$payload}))", null)
        }
    }

    /** 当前应用版本名（用于"每个版本最多提示一次通知权限"）。 */
    private fun appVersionName(): String = try {
        packageManager.getPackageInfo(packageName, 0).versionName ?: ""
    } catch (_: Exception) {
        ""
    }

    companion object { private const val PICK_DOCUMENTS = 3101 }
}
