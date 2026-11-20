package com.joymanga.app

import android.Manifest
import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.webkit.WebView
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InterruptedIOException
import java.util.UUID
import java.util.concurrent.Executors

internal class AndroidStorageHelper(
    private val activity: Activity,
    private val webViewProvider: () -> WebView?,
) {
    private data class Picker(val id: String, val mode: String, val exportPath: String? = null)

    private var pendingPicker: Picker? = null
    private val worker = Executors.newSingleThreadExecutor()
    @Volatile private var openFileHandlerReady = false

    fun scheduleStartupCleanup(isColdStart: Boolean) {
        if (!isColdStart) return
        activity.submitTo(worker) {
            // 仅真正冷启动且不在活跃暂存列表内时清理残留，防止旋转屏幕重建误删正在拷贝的暂存目录
            try {
                importRoot().listFiles()
                    ?.filter { !AppState.isStagingActive(it.name) }
                    ?.forEach { it.deleteRecursively() }
            } catch (_: Exception) {}
            try {
                openRoot().listFiles()
                    ?.filter {
                        !AppState.isStagingActive(it.name) &&
                            it.lastModified() < System.currentTimeMillis() - 10 * 60 * 1000
                    }
                    ?.forEach { it.deleteRecursively() }
            } catch (_: Exception) {}
            try { File(activity.cacheDir, "android-imports").deleteRecursively() } catch (_: Exception) {}
        }
    }

    fun onWebViewCreated() {
        // 新 WebView 的页面还没挂载监听器，等前端再次通知后再补发
        openFileHandlerReady = false
    }

    fun destroy() {
        worker.shutdownNow()
    }

    /**
     * 「用 JoyManga 打开 CBZ」：把外部文件复制到导入暂存区，再通知前端导入。
     * 前端监听器注册之前先入队，由 `notifyOpenFileReady` 按到达顺序补发。
     */
    fun handleViewIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val uri = intent.data ?: return
        intent.action = null
        val dispatchNow = synchronized(AppState.pendingViewUris) {
            if (openFileHandlerReady) {
                true
            } else {
                AppState.pendingViewUris.addLast(uri)
                false
            }
        }
        if (dispatchNow) dispatchViewUri(uri)
    }

    fun libraryPath(): String = libraryRoot().absolutePath

    fun downloadsPath(): String = publicLibraryRoot().absolutePath

    fun hasAllFilesAccess(): Boolean =
        when {
            // isExternalStorageManager 是 API 30 新增，API 29 调用会抛 NoSuchMethodError
            Build.VERSION.SDK_INT >= 30 -> Environment.isExternalStorageManager()
            Build.VERSION.SDK_INT < 29 ->
                activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
            // Android 10 既没有 MANAGE_EXTERNAL_STORAGE，WRITE_EXTERNAL_STORAGE 又被 maxSdkVersion=28 屏蔽
            else -> false
        }

    fun supportsPublicStorage(): Boolean =
        Build.VERSION.SDK_INT >= 30 || Build.VERSION.SDK_INT < 29

    fun requestAllFilesAccess() {
        activity.runOnUiThread {
            when {
                Build.VERSION.SDK_INT >= 30 -> {
                    try {
                        activity.startActivity(
                            Intent(
                                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                                Uri.parse("package:${activity.packageName}"),
                            ),
                        )
                    } catch (_: Exception) {
                        try {
                            activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                        } catch (_: Exception) {}
                    }
                }
                Build.VERSION.SDK_INT < 29 ->
                    activity.requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), REQUEST_STORAGE_PERMISSION)
                // API 29：没有任何可授予的公共存储权限，前端已隐藏该选项
            }
        }
    }

    fun canWritePublicDownloads(): Boolean = try {
        val dir = publicLibraryRoot()
        if (!dir.exists() && !dir.mkdirs()) {
            false
        } else {
            val probe = File(dir, ".write_test_${UUID.randomUUID()}")
            probe.writeText("test")
            probe.delete()
            true
        }
    } catch (_: Exception) {
        false
    }

    fun notifyOpenFileReady() {
        val pending = synchronized(AppState.pendingViewUris) {
            openFileHandlerReady = true
            val queued = AppState.pendingViewUris.toList()
            AppState.pendingViewUris.clear()
            queued
        }
        pending.forEach { dispatchViewUri(it) }
    }

    fun pickDocuments(id: String, mode: String) {
        if (!validId(id)) return
        if (mode !in listOf("cbz", "backup", "directory")) {
            result(id, error = "文件选择模式无效")
            return
        }
        activity.runOnUiThread { beginPicker(Picker(id, mode)) }
    }

    fun discardImports(id: String) {
        if (validId(id)) {
            cleanupStagingDir(id, openDir = File(openRoot(), id), importDir = File(importRoot(), id))
        }
    }

    fun exportDocuments(id: String, path: String) {
        if (!validId(id)) return
        if (Build.VERSION.SDK_INT >= 29) {
            if (!activity.submitTo(worker) {
                    try { result(id, exported = publishDownloads(File(path))) }
                    catch (error: Exception) { result(id, error = error.message ?: "导出失败") }
                }
            ) {
                result(id, error = "应用正在关闭，已取消导出")
            }
        } else {
            activity.runOnUiThread { beginPicker(Picker(id, "export-directory", path)) }
        }
    }

    /** 运行时权限结果回传：API 29 以下的存储授权没有可见性变化事件，前端靠这个事件完成引导。 */
    fun onRequestPermissionsResult(requestCode: Int, grantResults: IntArray) {
        if (requestCode != REQUEST_STORAGE_PERMISSION) return
        val granted = grantResults.isNotEmpty() && grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        activity.runOnUiThread {
            webViewProvider()?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('jm-permission-result', {detail:{request:'storage',granted:$granted}}))",
                null,
            )
        }
    }

    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode != PICK_DOCUMENTS) return
        val picker = pendingPicker ?: return
        pendingPicker = null
        if (resultCode != Activity.RESULT_OK || data == null) {
            result(picker.id, cancelled = true)
            return
        }
        val uris = buildList {
            data.clipData?.let { clip -> for (index in 0 until clip.itemCount) add(clip.getItemAt(index).uri) }
            if (isEmpty()) data.data?.let { add(it) }
        }
        if (uris.isEmpty()) {
            result(picker.id, cancelled = true)
            return
        }
        val isStagingPicker = picker.mode != "export-directory"
        if (isStagingPicker) AppState.markStagingActive(picker.id)
        val submitted = activity.submitTo(worker) {
            try {
                if (picker.mode == "export-directory") {
                    val uri = uris.first()
                    val count = publishToTree(File(picker.exportPath!!), uri)
                    result(picker.id, exported = count)
                    return@submitTo
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
                if (isStagingPicker) {
                    File(importRoot(), picker.id).deleteRecursively()
                    AppState.unmarkStagingActive(picker.id)
                }
                result(picker.id, error = error.message ?: "读取所选文件失败")
            }
        }
        if (!submitted) {
            if (isStagingPicker) AppState.unmarkStagingActive(picker.id)
            result(picker.id, error = "应用正在关闭，已取消读取所选文件")
        }
    }

    private fun dispatchViewUri(uri: Uri) {
        val requestId = UUID.randomUUID().toString()
        AppState.markStagingActive(requestId)
        val submitted = activity.submitTo(worker) {
            val staging = File(openRoot(), requestId)
            try {
                val name = safeFileName(documentName(uri))
                require(name.endsWith(".cbz", ignoreCase = true)) { "只支持打开 CBZ 文件" }
                // 外部打开的漫画可能很大，cacheDir 在存储紧张时会被系统清理，
                // 放到 filesDir 下的专门目录，导入完成后由前端调 discardImports 删除。
                staging.mkdirs()
                val target = File(staging, name)
                copyUri(uri, target, 2L * 1024 * 1024 * 1024)
                activity.runOnUiThread {
                    val view = webViewProvider()
                    if (view == null || activity.isDestroyed || activity.isFinishing) {
                        cleanupStagingDir(requestId, openDir = staging, importDir = null)
                        return@runOnUiThread
                    }
                    view.evaluateJavascript(
                        "window.dispatchEvent(new CustomEvent('jm-open-file', {detail:{id:${JSONObject.quote(requestId)},path:${JSONObject.quote(target.absolutePath)}}}))",
                        null,
                    )
                }
            } catch (error: Exception) {
                staging.deleteRecursively()
                AppState.unmarkStagingActive(requestId)
                activity.runOnUiThread {
                    if (activity.isDestroyed || activity.isFinishing) return@runOnUiThread
                    android.widget.Toast.makeText(
                        activity,
                        "打开文件失败：${error.message ?: "未知错误"}",
                        android.widget.Toast.LENGTH_LONG,
                    ).show()
                }
            }
        }
        if (!submitted) {
            AppState.unmarkStagingActive(requestId)
        }
    }

    private fun cleanupStagingDir(requestId: String, openDir: File?, importDir: File?) {
        val clean: () -> Unit = {
            try {
                openDir?.deleteRecursively()
                importDir?.deleteRecursively()
            } finally {
                AppState.unmarkStagingActive(requestId)
            }
        }
        if (!activity.submitTo(worker, allowDuringFinish = true, clean)) {
            Thread(Runnable { clean() }, "jm-staging-cleanup").start()
        }
    }

    private fun result(
        id: String,
        paths: List<String>? = null,
        error: String? = null,
        cancelled: Boolean = false,
        exported: Int? = null,
    ) {
        val payload = JSONObject().put("requestId", id).put("cancelled", cancelled)
        if (paths != null) payload.put("paths", JSONArray(paths))
        if (error != null) payload.put("error", error)
        if (exported != null) payload.put("exported", exported)
        activity.runOnUiThread {
            val view = webViewProvider()
            if (view == null || activity.isDestroyed || activity.isFinishing) {
                if (paths != null) {
                    cleanupStagingDir(id, openDir = File(openRoot(), id), importDir = File(importRoot(), id))
                }
                return@runOnUiThread
            }
            view.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('jm-mobile-result', {detail:$payload}))",
                null,
            )
        }
    }

    private fun libraryRoot() =
        File(activity.getExternalFilesDir(null) ?: activity.filesDir, "library").apply { mkdirs() }

    // 导入暂存放 filesDir 而不是 cacheDir：大文件复制耗时较长，期间系统可能清理缓存目录，
    // 导致复制到一半的文件被删。导入结束后由前端 discardImports 清理。
    private fun importRoot() = File(activity.filesDir, "android-imports").apply { mkdirs() }
    private fun openRoot() = File(activity.filesDir, "android-opens").apply { mkdirs() }
    private fun publicLibraryRoot() =
        File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "JoyManga")
    private fun validId(id: String): Boolean = try { UUID.fromString(id); true } catch (_: Exception) { false }

    @Suppress("DEPRECATION")
    private fun beginPicker(picker: Picker) {
        if (pendingPicker != null) {
            result(picker.id, error = "已有文件选择窗口，请先完成或取消")
            return
        }
        pendingPicker = picker
        val tree = picker.mode == "directory" || picker.mode == "export-directory"
        val intent = if (tree) Intent(Intent.ACTION_OPEN_DOCUMENT_TREE) else Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = if (picker.mode == "backup") "application/json" else "*/*"
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, picker.mode == "cbz")
            if (picker.mode == "cbz") {
                putExtra(
                    Intent.EXTRA_MIME_TYPES,
                    arrayOf("application/zip", "application/x-cbz", "application/vnd.comicbook+zip", "application/octet-stream"),
                )
            }
        }
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        if (picker.mode == "export-directory") intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        try {
            activity.startActivityForResult(intent, PICK_DOCUMENTS)
        } catch (error: Exception) {
            pendingPicker = null
            result(picker.id, error = error.message ?: "无法打开系统文件选择器")
        }
    }

    private fun documentName(uri: Uri): String =
        activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) ?: "file" else "file"
        } ?: "file"

    private fun queryDocumentSize(uri: Uri): Long? = try {
        activity.contentResolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use {
            if (it.moveToFirst() && !it.isNull(0)) it.getLong(0).takeIf { size -> size >= 0L } else null
        }
    } catch (_: Exception) {
        null
    }

    private fun uniqueFile(parent: File, name: String): File {
        var file = File(parent, name)
        if (file.exists()) file = File(parent, UUID.randomUUID().toString().take(8) + "-" + name)
        return file
    }

    private fun copyUri(uri: Uri, target: File, limit: Long, knownSize: Long? = null) {
        if (Thread.currentThread().isInterrupted) throw InterruptedIOException("导入已取消")
        val parent = target.parentFile ?: throw IOException("暂存路径无效")
        val declaredSize = knownSize?.takeIf { it >= 0L } ?: queryDocumentSize(uri)
        val freeSpace = parent.usableSpace
        if (declaredSize != null) {
            require(declaredSize <= limit) { "所选文件超过大小上限" }
            require(freeSpace >= declaredSize + STAGING_RESERVE_BYTES) { "设备可用存储空间不足，请清理空间后重试" }
        } else {
            require(freeSpace >= STAGING_RESERVE_BYTES) { "设备可用存储空间不足，请清理空间后重试" }
        }
        val part = File(parent, ".part-" + UUID.randomUUID())
        try {
            activity.contentResolver.openInputStream(uri).use { input ->
                require(input != null) { "文件提供器无法读取所选文件" }
                FileOutputStream(part).use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    var lastSpaceCheck = 0L
                    while (true) {
                        if (Thread.currentThread().isInterrupted) throw InterruptedIOException("导入已取消")
                        val count = input.read(buffer)
                        if (count < 0) break
                        total += count
                        require(total <= limit) { "所选文件超过大小上限" }
                        if (total - lastSpaceCheck >= SPACE_RECHECK_INTERVAL_BYTES) {
                            lastSpaceCheck = total
                            require(parent.usableSpace >= STAGING_RESERVE_BYTES) { "设备可用存储空间不足，已中止导入" }
                        }
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
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        activity.contentResolver.query(children, projection, null, null, null)?.use { cursor ->
            while (cursor.moveToNext()) {
                if (Thread.currentThread().isInterrupted) throw InterruptedIOException("导入已取消")
                require(++counter[0] <= 10000) { "目录中文件过多" }
                val uri = DocumentsContract.buildDocumentUriUsingTree(tree, cursor.getString(0))
                val name = safeFileName(cursor.getString(1) ?: "file")
                val type = cursor.getString(2)
                val size = if (!cursor.isNull(3)) cursor.getLong(3) else null
                if (type == DocumentsContract.Document.MIME_TYPE_DIR) {
                    val directory = uniqueFile(target, name).apply { mkdirs() }
                    copyTree(tree, uri, directory, depth + 1, counter)
                } else if (name.endsWith(".cbz", true) || listOf(".jpg", ".jpeg", ".png", ".webp").any { name.endsWith(it, true) }) {
                    copyUri(
                        uri,
                        uniqueFile(target, name),
                        if (name.endsWith(".cbz", true)) 2 * 1024 * 1024 * 1024L else 32 * 1024 * 1024L,
                        knownSize = size,
                    )
                }
            }
        } ?: throw IOException("文件提供器无法读取目录")
    }

    private fun exportFiles(path: File): List<File> {
        val canonical = path.canonicalFile
        // 书库可能是应用专用目录、公共 Download/JoyManga，或用户通过目录选择器挑的
        // 其他公共目录；三者都允许导出（无存储权限时公共路径的读取会自然失败）。
        // 只发布 cbz/json/txt，避免把目录里的其他文件一起复制出去。
        val allowed = listOfNotNull(
            activity.filesDir,
            activity.getExternalFilesDir(null),
            Environment.getExternalStorageDirectory(),
        ).map { it.canonicalFile }
        require(allowed.any { canonical.path == it.path || canonical.path.startsWith(it.path + File.separator) }) {
            "请先将文件导出到应用专用目录或公共存储"
        }
        val files = if (canonical.isFile) listOf(canonical) else canonical.walkTopDown().filter { it.isFile }.take(1001).toList()
        require(files.size <= 1000) { "单次最多导出 1000 个文件" }
        return files.filter { file -> listOf("cbz", "json", "txt").contains(file.extension.lowercase()) }
    }

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
            val uri = activity.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                ?: throw IOException("创建公共下载文件失败")
            try {
                activity.contentResolver.openFileDescriptor(uri, "w").use { descriptor ->
                    require(descriptor != null) { "无法写入公共文件" }
                    FileOutputStream(descriptor.fileDescriptor).use { output ->
                        file.inputStream().use { it.copyTo(output) }
                        output.fd.sync()
                    }
                }
                require(activity.contentResolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null) == 1) {
                    "发布公共文件失败"
                }
            } catch (error: Exception) {
                activity.contentResolver.delete(uri, null, null)
                throw error
            }
        }
        return files.size
    }

    private fun publishToTree(path: File, tree: Uri): Int {
        val files = exportFiles(path)
        require(files.isNotEmpty()) { "没有可导出的文件" }
        val parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        for (file in files) {
            val staged = DocumentsContract.createDocument(activity.contentResolver, parent, mime(file), ".part-" + UUID.randomUUID())
                ?: throw IOException("文件提供器无法创建文件")
            try {
                activity.contentResolver.openOutputStream(staged, "w").use { output ->
                    require(output != null)
                    file.inputStream().use { it.copyTo(output) }
                }
                require(DocumentsContract.renameDocument(activity.contentResolver, staged, file.name) != null) {
                    "此位置不支持完整文件提交，请换一个系统文件夹"
                }
            } catch (error: Exception) {
                DocumentsContract.deleteDocument(activity.contentResolver, staged)
                throw error
            }
        }
        return files.size
    }

    companion object {
        private const val PICK_DOCUMENTS = 3101
        private const val REQUEST_STORAGE_PERMISSION = 102
        private const val STAGING_RESERVE_BYTES = 64L * 1024 * 1024L
        private const val SPACE_RECHECK_INTERVAL_BYTES = 4L * 1024 * 1024L
    }
}
