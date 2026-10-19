package com.joymanga.app

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.*
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.Uri
import android.os.*
import android.provider.*
import android.webkit.*
import android.view.inputmethod.InputMethodManager
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
    private var visible = false
    private var activeDownloads = 0
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
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() { dispatchBack() }
        })
        worker.execute {
            try { importRoot().listFiles()?.forEach { it.deleteRecursively() } } catch (_: Exception) {}
        }
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        appWebView = webView
        webView.addJavascriptInterface(AndroidBridge(), "JmAndroid")
        registerNetworkCallback()
    }

    /** 网络切换（Wi-Fi <-> 移动数据）时把"是否计费"推给前端，供「仅 Wi-Fi 下载」判断。 */
    private fun registerNetworkCallback() {
        try {
            val manager = getSystemService(ConnectivityManager::class.java)
            val request = NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .build()
            manager.registerNetworkCallback(request, object : ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                    pushMetered(!capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED))
                }
                override fun onLost(network: Network) { pushMetered(true) }
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
        // 离开应用时清掉常亮，避免阅读器关闭回调丢失后一直不熄屏
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
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
    private fun importRoot() = File(cacheDir, "android-imports").apply { mkdirs() }
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
        val allowed = listOfNotNull(filesDir, getExternalFilesDir(null)).map { it.canonicalFile }
        require(allowed.any { canonical.path == it.path || canonical.path.startsWith(it.path + File.separator) }) { "请先将文件导出到应用专用目录" }
        val files = if (canonical.isFile) listOf(canonical) else canonical.walkTopDown().filter { it.isFile }.take(1001).toList()
        require(files.size <= 1000) { "单次最多导出 1000 个文件" }
        return files.filter { file -> listOf("cbz", "json").contains(file.extension.lowercase()) }
    }

    private fun mime(file: File) = when (file.extension.lowercase()) { "json" -> "application/json"; else -> "application/zip" }

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

    inner class AndroidBridge {
        @JavascriptInterface fun libraryPath(): String = libraryRoot().absolutePath
        @JavascriptInterface fun exitApp() { runOnUiThread { moveTaskToBack(true) } }
        @JavascriptInterface fun pickDocuments(id: String, mode: String) {
            if (!validId(id)) return
            if (mode !in listOf("cbz", "backup", "directory")) { result(id, error = "文件选择模式无效"); return }
            runOnUiThread { beginPicker(Picker(id, mode)) }
        }
        @JavascriptInterface fun discardImports(id: String) {
            if (validId(id)) worker.execute { File(importRoot(), id).deleteRecursively() }
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
                updateDownloadService()
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
    /** 当前应用版本名（用于"每个版本最多提示一次通知权限"）。 */
    private fun appVersionName(): String = try {
        packageManager.getPackageInfo(packageName, 0).versionName ?: ""
    } catch (_: Exception) {
        ""
    }

    companion object { private const val PICK_DOCUMENTS = 3101 }
}
