package com.joymanga.app

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
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
    // 安装包的清理/删除单独占一个线程：大文件删除偶尔要等文件系统，不能挡住导入导出的暂存清理。
    private val updateWorker = Executors.newSingleThreadExecutor()
    private var visible = false
    private var activeDownloads = 0
    private val handler = Handler(Looper.getMainLooper())
    private var serviceUpdateScheduled = false
    // 拉起安装器后待清理的安装包。不能用 onStart 当「安装已结束」的信号——MIUI/HyperOS
    // 会在确认后转后台继续安装并立刻交还前台，那时删包会让安装失败。改为依据真实结果：
    // 安装成功走 PACKAGE_REPLACED 广播，用户取消/失败则由 10 分钟兜底。
    private var pendingUpdateApkCleanup: File? = null
    private val updateApkCleanupTimeout = Runnable { runUpdateApkCleanup("超时未收到安装结果") }
    // PackageInstaller 会话结果的回传广播。用应用内私有 action + 动态注册，
    // 不写进清单，其他应用无法伪造「安装成功/失败」。
    private var updateInstallReceiverRegistered = false
    // WebView 创建早于页面加载，事件在 Vue 注册监听器之前派发就会丢失。这里改成
    // 前端注册监听器后主动通知（notifyOpenFileReady），之后再按顺序补发。用队列而不是
    // 单槽位，连续「用 JoyManga 打开」多个 CBZ 时不会只剩最后一个。
    private val pendingViewUris = ArrayDeque<Uri>()
    @Volatile private var openFileHandlerReady = false
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
            // 安装成功时系统会杀掉本进程，onStart 的清理来不及执行，安装包会一直留着。
            // 启动时兜底清掉 1 小时前的旧包：此时上一次安装早已结束，不会影响安装器读取。
            try {
                File(cacheDir, "updates").listFiles()
                    ?.filter { it.lastModified() < System.currentTimeMillis() - 60 * 60 * 1000 }
                    ?.forEach { it.delete() }
            } catch (_: Exception) {}
        }
        // 冷启动时由「用 JoyManga 打开 CBZ」拉起：WebView 就绪后会自动导入
        handleViewIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleViewIntent(intent)
        // 点「安装包已下载完成」通知回来时 Activity 可能已经在前台（没有 onStart），
        // 这里补一次；待安装包取走后就不会再重复拉起安装器。
        installPendingUpdate()
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        appWebView = webView
        // 新 WebView 的页面还没挂载监听器，等前端再次通知后再补发
        openFileHandlerReady = false
        webView.addJavascriptInterface(AndroidBridge(), "JmAndroid")
        registerNetworkCallback()
    }

    /**
     * 「用 JoyManga 打开 CBZ」：把外部文件复制到导入暂存区，再通知前端导入。
     * 前端监听器注册之前先入队，由 `notifyOpenFileReady` 按到达顺序补发。
     */
    private fun handleViewIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val uri = intent.data ?: return
        intent.action = null
        val dispatchNow = synchronized(pendingViewUris) {
            if (openFileHandlerReady) {
                true
            } else {
                pendingViewUris.addLast(uri)
                false
            }
        }
        if (dispatchNow) dispatchViewUri(uri)
    }

    private fun dispatchViewUri(uri: Uri) {
        val requestId = UUID.randomUUID().toString()
        worker.execute {
            try {
                val name = safeFileName(documentName(uri))
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
        AppState.foreground = true
        updateDownloadService()
        pushMetered(getSystemService(ConnectivityManager::class.java).isActiveNetworkMetered)
        installPendingUpdate()
    }
    override fun onStop() {
        visible = false
        AppState.foreground = false
        // 服务要能继续把「取消当前」转发给前端，所以不在这里清 activity；
        // 是否允许拉起界面改由 AppState.foreground 判断（后台不能 startActivity）。
        // 界面真正销毁时在 onDestroy 里清空。
        // 不要在这里清 KEEP_SCREEN_ON：窗口标志只在窗口可见时生效，不可见时本来就不会
        // 阻止息屏；清掉后前端不会再设置（watcher 只在配置/阅读状态变化时触发），
        // 用户切出去一次再回来，本次阅读就一直会息屏。
        super.onStop()
    }

    override fun onDestroy() {
        // 延时任务持有 Runnable，进而持有本 Activity；不摘掉会让主线程队列钉住
        // 整个 Activity 与 WebView 直到超时。工作线程同样在这里回收。
        handler.removeCallbacks(updateApkCleanupTimeout)
        pendingUpdateApkCleanup = null
        // 接收器持有 Activity 的隐式引用，不注销会随 Activity 泄漏
        if (updateInstallReceiverRegistered) {
            try { unregisterReceiver(updateInstallReceiver) } catch (_: Exception) {}
            updateInstallReceiverRegistered = false
        }
        worker.shutdownNow()
        updateWorker.shutdownNow()
        if (AppState.activity === this) AppState.activity = null
        AppState.foreground = false
        super.onDestroy()
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
                    val target = File(staging, safeFileName(documentName(root))).apply { mkdirs() }
                    val counter = intArrayOf(0)
                    copyTree(source, root, target, 0, counter)
                    listOf(target.absolutePath)
                } else uris.map { uri ->
                    val name = safeFileName(documentName(uri))
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
                val name = safeFileName(cursor.getString(1) ?: "file")
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
            // 按 Android 的定义，持有前台服务的应用算「前台」，可以继续给它发 start；
            // 只有「没有可见界面、也没有前台服务在跑」时才不能 startForegroundService。
            if (visible || AppState.downloadServiceRunning) {
                if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent) else startService(intent)
            } else {
                notifyRunningInBackground()
            }
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "下载前台服务启动失败", error)
            // 前台服务被系统拒绝时至少要有一条通知，否则下载在后台静默进行，用户毫无感知
            try { notifyRunningInBackground() } catch (_: Exception) {}
        }
    }

    /** 不能起前台服务时的退路：只更新通知。已运行的服务会接收更新，不会新建服务。 */
    private fun notifyRunningInBackground() {
        // 服务可能从未成功创建过，渠道得在这里自己补上；否则 API 26+ 上通知会被静默丢弃
        DownloadService.ensureChannel(this)
        getSystemService(NotificationManager::class.java).notify(DownloadService.ID,
            (if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, DownloadService.CHANNEL) else Notification.Builder(this))
                .setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle("正在下载 $activeDownloads 个章节")
                .setContentText(downloadSpeed).setOngoing(true).setOnlyAlertOnce(true)
                .setContentIntent(android.app.PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE)).build())
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
        /** 前端已注册 `jm-open-file` 监听器：把等待中的外部文件按顺序补发。 */
        @JavascriptInterface fun notifyOpenFileReady() {
            val pending = synchronized(pendingViewUris) {
                openFileHandlerReady = true
                val queued = pendingViewUris.toList()
                pendingViewUris.clear()
                queued
            }
            pending.forEach { dispatchViewUri(it) }
        }
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
            // 下载放在前台服务里：应用切到后台后，Activity 里的普通线程没有前台服务
            // 保护，进程随时会被回收；Activity 销毁时还会被 shutdownNow() 直接打断。
            // 服务内部对重复点击做了去重，这里不再需要 updateDownloading 标志。
            val intent = Intent(this@MainActivity, UpdateDownloadService::class.java)
                .putExtra(UpdateDownloadService.EXTRA_URL, url)
                .putExtra(UpdateDownloadService.EXTRA_FILE, fileName)
            try {
                if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent) else startService(intent)
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "更新下载服务启动失败", error)
                emitUpdateEvent("error", message = "无法开始下载更新：${error.message ?: "未知错误"}")
            }
        }
        /**
         * 界面重新可见时取一次更新下载的实时状态。
         *
         * 下载在服务里进行，期间 WebView 可能被暂停或重建，错过 `jm-update-progress`
         * 事件；前端每次回到前台都主动拉一次，界面就不会停在「未开始」或旧进度上。
         */
        @JavascriptInterface fun updateDownloadState() {
            if (UpdateDownloadService.isRunning()) {
                emitUpdateEvent("progress", percent = UpdateDownloadService.progress())
                return
            }
            // 后台期间下载已经结束：把结局补发给界面。否则回来后会一直停在
            // 「下载中 x%」或「安装包正在下载」上，按钮再也点不动。
            val outcome = UpdateDownloadService.takeOutcome()
            if (outcome != null) {
                emitUpdateEvent(outcome.first, percent = if (outcome.first == "installer") 100 else -1, message = outcome.second)
            } else {
                emitUpdateEvent("idle")
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
                // 开始下载的这一刻就要把前台服务拉起来：合并窗口只有 750ms，但用户完全可能
                // 点完下载立刻切到后台，而 Android 不允许后台应用新建前台服务——那之后整个
                // 下载都只是普通后台进程，被系统回收就断了。
                if (activeDownloads > 0 && !wasActive) updateDownloadService()
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
        /**
         * 阅读器沉浸式全屏：隐藏状态栏与导航栏，内容延伸到屏幕边缘。
         *
         * 用 WindowInsetsController 而不是旧的 systemUiVisibility 标志：后者在
         * Android 11+ 已废弃，且不会让内容真正铺到刘海/手势条下方。关闭时恢复，
         * 退出阅读器页面被销毁时也要调用，否则状态栏会一直不回来。
         */
        @JavascriptInterface fun setImmersiveMode(enabled: Boolean) {
            runOnUiThread {
                val controller = WindowCompat.getInsetsController(window, window.decorView)
                if (enabled) {
                    // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE：从边缘滑动可临时唤出系统栏
                    controller.systemBarsBehavior =
                        WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                    controller.hide(WindowInsetsCompat.Type.systemBars())
                } else {
                    controller.show(WindowInsetsCompat.Type.systemBars())
                }
            }
        }
        /** 音量键翻页：由前端根据设置开启，原生侧只在开启时接管按键。 */
        @JavascriptInterface fun setVolumeKeyPaging(enabled: Boolean) {
            runOnUiThread { volumeKeyPaging = enabled }
        }
    }
    /**
     * 供 [UpdateDownloadService] 调用：应用在前台时下载完成，直接拉起安装器。
     *
     * 服务已经确认过安装未知应用的权限，这里不必重复通知前端「permission」，
     * 但权限可能在下载期间被用户关掉，所以仍保留 [installApk] 里的检查。
     */
    internal fun installDownloadedUpdate(apk: File) {
        runOnUiThread { installApk(apk) }
    }

    /**
     * 后台下载完成、用户点通知或重新打开应用时，把待安装的安装包交给系统安装器。
     *
     * 只在界面可见时调用：Android 10+ 后台应用 `startActivity` 会被系统忽略。
     */
    private fun installPendingUpdate() {
        // 待安装记录可能来自上一次进程（下载完成后应用被系统回收）：只在界面可见、
        // 且下载服务当前没有任务时才弹出，避免刚冷启动就弹安装器。
        // onNewIntent 也可能先于 onStart 到达，此时后台起 Activity 会被系统拒绝，
        // 交给紧接着的 onStart 处理。
        if (!visible || UpdateDownloadService.isRunning()) return
        val apk = UpdateDownloadService.takePendingInstall(this) ?: return
        UpdateDownloadService.clearReadyNotification(this)
        installDownloadedUpdate(apk)
    }

    /**
     * 把下载好的 APK 交给系统安装。
     *
     * 走 `PackageInstaller.Session` 而不是 `ACTION_VIEW`：前者把 APK 直接写进安装会话，
     * 由框架层的 PackageInstallerService 接管，不再经过小米等 ROM 自绘的安装器 Activity。
     * MIUI/HyperOS 桌面在安装期间会额外画一个「安装中」占位图标，那是启动器对
     * `PACKAGE_REMOVED(EXTRA_REPLACING=true)` 的反应；换用 Session 后是否还会出现，
     * 取决于该 ROM 是挂在安装器界面还是挂在包状态变化上，需要在真机上验证。
     *
     * 用户确认弹窗仍然由系统给出（非系统签名应用无法静默安装）。
     */
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
            commitInstallSession(apk)
            emitUpdateEvent("installer", message = "请在系统弹窗中确认安装")
            scheduleUpdateApkCleanup(apk)
        } catch (error: Exception) {
            // 各 ROM 对 PackageInstaller 的限制不一致（部分定制系统禁用第三方 session、
            // 或要求额外权限）。会话这条路失败时回退到 ACTION_VIEW，保证任何机型都装得上。
            android.util.Log.w("JoyManga", "PackageInstaller 会话提交失败，回退到系统安装器", error)
            try {
                fallbackInstallViaIntent(apk)
                emitUpdateEvent("installer", message = "请在系统安装器中确认安装")
                scheduleUpdateApkCleanup(apk)
            } catch (fallbackError: Exception) {
                emitUpdateEvent("error", message = "无法启动安装：${fallbackError.message ?: "未知错误"}")
            }
        }
    }

    /**
     * 用 PackageInstaller 会话提交安装。
     *
     * 成功返回时会话已提交，结果稍后由 [updateInstallReceiver] 回传；
     * 抛出异常表示这台设备不支持该路径，调用方应回退。
     */
    private fun commitInstallSession(apk: File) {
        val installer = packageManager.packageInstaller
        val params = android.content.pm.PackageInstaller.SessionParams(
            android.content.pm.PackageInstaller.SessionParams.MODE_FULL_INSTALL,
        ).apply {
            // 目标包名写进会话参数，安装器据此识别这是「更新同包」而非新装，
            // 也是桌面能正确复用原图标条目的前提。
            setAppPackageName(packageName)
        }
        val sessionId = installer.createSession(params)
        // 结果通过 PendingIntent 回传。用显式 Intent 指向本进程的广播接收器，
        // 不注册清单项，避免导出组件被其他应用伪造安装结果。
        // 接收器必须在 commit 之前注册：会话结果可能立刻回传，晚注册会漏掉
        // STATUS_PENDING_USER_ACTION，用户就永远看不到确认弹窗。
        val resultIntent = Intent(UPDATE_INSTALL_ACTION).setPackage(packageName)
        val flags = if (Build.VERSION.SDK_INT >= 31) {
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pending = PendingIntent.getBroadcast(this, sessionId, resultIntent, flags)
        if (!updateInstallReceiverRegistered) {
            registerReceiver(
                updateInstallReceiver,
                IntentFilter(UPDATE_INSTALL_ACTION),
                if (Build.VERSION.SDK_INT >= 33) Context.RECEIVER_NOT_EXPORTED else 0,
            )
            updateInstallReceiverRegistered = true
        }
        installer.openSession(sessionId).use { session ->
            apk.inputStream().use { input ->
                session.openWrite("base.apk", 0, apk.length()).use { output ->
                    input.copyTo(output)
                    // 必须 fsync：只写不刷会把截断的 APK 交给安装器，安装直接失败
                    session.fsync(output)
                }
            }
            session.commit(pending.intentSender)
        }
    }

    /** 回退路径：把 APK 经 FileProvider 授权后交给系统安装器 Activity。 */
    private fun fallbackInstallViaIntent(apk: File) {
        val uri = androidx.core.content.FileProvider.getUriForFile(this, "$packageName.fileprovider", apk)
        startActivity(Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        })
    }

    /** PackageInstaller 的会话结果回调；注册在 installApk 里，销毁时注销。 */
    private val updateInstallReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != UPDATE_INSTALL_ACTION) return
            val status = intent.getIntExtra(android.content.pm.PackageInstaller.EXTRA_STATUS, -1)
            when (status) {
                android.content.pm.PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                    // 系统还需要用户在弹窗里点一次「安装」：把确认界面拉起来
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)?.let { confirm ->
                        try {
                            startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                        } catch (error: Exception) {
                            emitUpdateEvent("error", message = "无法打开安装确认界面：${error.message ?: "未知错误"}")
                        }
                    }
                }
                android.content.pm.PackageInstaller.STATUS_SUCCESS -> {
                    // 成功时进程通常已被替换杀掉，这里多数情况跑不到，保留是为了覆盖
                    // 系统选择「不杀进程」的机型。安装包由启动兜底清理接手。
                    runUpdateApkCleanup("安装成功")
                }
                else -> {
                    val reason = intent.getStringExtra(android.content.pm.PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "安装未完成"
                    emitUpdateEvent("error", message = if (status == android.content.pm.PackageInstaller.STATUS_FAILURE_ABORTED) "已取消安装" else reason)
                    // 用户取消或安装失败：安装器不会再读这个文件，立刻清掉
                    runUpdateApkCleanup("安装未完成")
                }
            }
        }
    }

    /**
     * 拉起安装器后登记待清理的安装包。
     *
     * 安装器读取 APK 会持续到安装结束，不能在拉起后立刻删。删除时机有两个：
     * 安装成功后系统发出 `PACKAGE_REPLACED`（此时本进程通常已被杀掉，由启动兜底清理接手）；
     * 用户取消或安装失败时收不到广播，10 分钟后兜底删除。
     */
    private fun scheduleUpdateApkCleanup(apk: File) {
        // 上一次还没清理的安装包先删掉，避免被本次覆盖后再没人负责
        runUpdateApkCleanup("上一次更新未完成")
        pendingUpdateApkCleanup = apk
        handler.removeCallbacks(updateApkCleanupTimeout)
        handler.postDelayed(updateApkCleanupTimeout, 10 * 60 * 1000)
    }

    private fun runUpdateApkCleanup(reason: String) {
        val apk = pendingUpdateApkCleanup ?: return
        pendingUpdateApkCleanup = null
        handler.removeCallbacks(updateApkCleanupTimeout)
        updateWorker.execute {
            try {
                if (apk.delete()) android.util.Log.i("JoyManga", "已清理更新安装包（$reason）")
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "清理更新安装包失败", error)
            }
        }
    }

    /** 更新流程进度/结果回传前端；事件名与 `src/utils/androidStorage.ts` 的订阅保持一致。 */
    private fun emitUpdateEvent(state: String, percent: Int = -1, message: String = "") =
        AppState.emitUpdate(state, percent, message)

    /** 当前应用版本名（用于"每个版本最多提示一次通知权限"）。 */
    private fun appVersionName(): String = try {
        packageManager.getPackageInfo(packageName, 0).versionName ?: ""
    } catch (_: Exception) {
        ""
    }

    companion object {
        private const val PICK_DOCUMENTS = 3101
        /** PackageInstaller 会话结果广播的私有 action */
        private const val UPDATE_INSTALL_ACTION = "com.joymanga.app.UPDATE_INSTALL_RESULT"
    }
}
