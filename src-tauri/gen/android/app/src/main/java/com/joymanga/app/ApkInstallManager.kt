package com.joymanga.app

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import java.io.File
import java.util.concurrent.Executors

internal class ApkInstallManager(private val activity: Activity) {
    // 安装包的清理/删除单独占一个线程：大文件删除偶尔要等文件系统，不能挡住导入导出的暂存清理。
    private val updateWorker = Executors.newSingleThreadExecutor()
    private val handler = Handler(Looper.getMainLooper())

    // 拉起安装器后待清理的安装包。不能用 onStart 当「安装已结束」的信号——MIUI/HyperOS
    // 会在确认后转后台继续安装并立刻交还前台，那时删包会让安装失败。改为依据真实结果：
    // 安装成功走 PACKAGE_REPLACED 广播，用户取消/失败则由 10 分钟兜底。
    private var pendingUpdateApkCleanup: File? = null
    private val updateApkCleanupTimeout = Runnable { runUpdateApkCleanup("超时未收到安装结果") }

    // PackageInstaller 会话结果的回传广播。用应用内私有 action + 动态注册，
    // 不写进清单，其他应用无法伪造「安装成功/失败」。
    private var updateInstallReceiverRegistered = false

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
                            activity.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                        } catch (error: Exception) {
                            emitUpdateEvent("error", message = "无法打开安装确认界面：${error.message ?: "未知错误"}")
                        }
                    }
                }
                android.content.pm.PackageInstaller.STATUS_SUCCESS -> {
                    // 成功时进程通常已被替换杀掉，这里多数情况跑不到，保留是为了覆盖
                    // 系统选择「不杀进程」的机型。安装包由启动兜底清理接手。
                    UpdateDownloadService.clearOutcome()
                    runUpdateApkCleanup("安装成功")
                }
                else -> {
                    UpdateDownloadService.clearOutcome()
                    val reason = intent.getStringExtra(android.content.pm.PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "安装未完成"
                    emitUpdateEvent("error", message = if (status == android.content.pm.PackageInstaller.STATUS_FAILURE_ABORTED) "已取消安装" else reason)
                    // 用户取消或安装失败：安装器不会再读这个文件，立刻清掉
                    runUpdateApkCleanup("安装未完成")
                }
            }
        }
    }

    fun scheduleStartupCleanup() {
        val pendingPath = UpdateDownloadService.peekPendingInstallPath(activity)
        activity.submitTo(updateWorker) {
            // 安装成功时系统会杀掉本进程，onStart 的清理来不及执行，安装包会一直留着。
            // 启动时兜底清掉 1 小时前的旧包，但必须保留尚未取走的待安装后台包。
            try {
                File(activity.cacheDir, "updates").listFiles()
                    ?.filter {
                        it.absolutePath != pendingPath &&
                            it.lastModified() < System.currentTimeMillis() - 60 * 60 * 1000
                    }
                    ?.forEach { it.delete() }
            } catch (_: Exception) {}
        }
    }

    fun destroy() {
        handler.removeCallbacks(updateApkCleanupTimeout)
        pendingUpdateApkCleanup = null
        // 接收器持有 Activity 的隐式引用，不注销会随 Activity 泄漏
        if (updateInstallReceiverRegistered) {
            try { activity.unregisterReceiver(updateInstallReceiver) } catch (_: Exception) {}
            updateInstallReceiverRegistered = false
        }
        updateWorker.shutdownNow()
    }

    /**
     * 应用内下载 APK 并拉起系统安装器。
     *
     * 浏览器下载再返回应用很折腾；这里下到私有缓存目录，经 FileProvider 授权后
     * 交给 PackageInstaller（用户仍需在系统弹窗确认）。进度通过
     * `jm-update-progress` 事件回传。
     */
    fun downloadAndInstallApk(url: String, fileName: String) {
        if (!url.startsWith("https://")) {
            emitUpdateEvent("error", message = "更新地址无效")
            return
        }
        // 先确认能装再下载：否则几十 MB 的安装包下完才发现没有权限，白费流量
        if (Build.VERSION.SDK_INT >= 26 && !activity.packageManager.canRequestPackageInstalls()) {
            emitUpdateEvent("permission", message = "请在设置中允许 JoyManga 安装未知应用，然后重新点击更新")
            activity.runOnUiThread {
                try {
                    activity.startActivity(
                        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}")),
                    )
                } catch (error: Exception) {
                    android.util.Log.w("JoyManga", "无法打开安装未知应用设置页", error)
                }
            }
            return
        }
        // 下载放在前台服务里：应用切到后台后，Activity 里的普通线程没有前台服务
        // 保护，进程随时会被回收；Activity 销毁时还会被 shutdownNow() 直接打断。
        // 服务内部对重复点击做了去重，这里不再需要 updateDownloading 标志。
        val intent = Intent(activity, UpdateDownloadService::class.java)
            .putExtra(UpdateDownloadService.EXTRA_URL, url)
            .putExtra(UpdateDownloadService.EXTRA_FILE, fileName)
        try {
            if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(intent) else activity.startService(intent)
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "更新下载服务启动失败", error)
            emitUpdateEvent("error", message = "无法开始下载更新：${error.message ?: "未知错误"}")
        }
    }

    /**
     * 界面重新可见时取一次更新下载的实时状态。
     */
    fun syncDownloadState() {
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

    fun installDownloadedUpdate(apk: File) {
        activity.runOnUiThread { installApk(apk) }
    }

    /**
     * 后台下载完成、用户点通知或重新打开应用时，把待安装的安装包交给系统安装器。
     *
     * 只在界面可见时调用：Android 10+ 后台应用 `startActivity` 会被系统忽略。
     */
    fun installPendingUpdate(isVisible: Boolean) {
        if (!isVisible || UpdateDownloadService.isRunning()) return
        val apk = UpdateDownloadService.takePendingInstall(activity)
        UpdateDownloadService.clearReadyNotification(activity)
        if (apk != null) installDownloadedUpdate(apk)
    }

    private fun installApk(apk: File) {
        if (Build.VERSION.SDK_INT >= 26 && !activity.packageManager.canRequestPackageInstalls()) {
            UpdateDownloadService.clearOutcome()
            emitUpdateEvent("permission", message = "请在设置中允许 JoyManga 安装未知应用，然后重新点击更新")
            try {
                activity.startActivity(
                    Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}")),
                )
            } catch (error: Exception) {
                android.util.Log.w("JoyManga", "无法打开安装未知应用设置页", error)
            }
            return
        }
        try {
            commitInstallSession(apk)
            UpdateDownloadService.clearOutcome()
            emitUpdateEvent("installer", message = "请在系统弹窗中确认安装")
            scheduleUpdateApkCleanup(apk)
        } catch (error: Exception) {
            // 各 ROM 对 PackageInstaller 的限制不一致（部分定制系统禁用第三方 session、
            // 或要求额外权限）。会话这条路失败时回退到 ACTION_VIEW，保证任何机型都装得上。
            android.util.Log.w("JoyManga", "PackageInstaller 会话提交失败，回退到系统安装器", error)
            try {
                fallbackInstallViaIntent(apk)
                UpdateDownloadService.clearOutcome()
                emitUpdateEvent("installer", message = "请在系统安装器中确认安装")
                scheduleUpdateApkCleanup(apk)
            } catch (fallbackError: Exception) {
                UpdateDownloadService.clearOutcome()
                emitUpdateEvent("error", message = "无法启动安装：${fallbackError.message ?: "未知错误"}")
            }
        }
    }

    /**
     * 用 PackageInstaller 会话提交安装。
     */
    private fun commitInstallSession(apk: File) {
        val installer = activity.packageManager.packageInstaller
        val params = android.content.pm.PackageInstaller.SessionParams(
            android.content.pm.PackageInstaller.SessionParams.MODE_FULL_INSTALL,
        ).apply {
            // 目标包名写进会话参数，安装器据此识别这是「更新同包」而非新装，
            // 也是桌面能正确复用原图标条目的前提。
            setAppPackageName(activity.packageName)
        }
        val sessionId = installer.createSession(params)
        // 结果通过 PendingIntent 回传。用显式 Intent 指向本进程的广播接收器，
        // 不注册清单项，避免导出组件被其他应用伪造安装结果。
        // 接收器必须在 commit 之前注册：会话结果可能立刻回传，晚注册会漏掉
        // STATUS_PENDING_USER_ACTION，用户就永远看不到确认弹窗。
        val resultIntent = Intent(UPDATE_INSTALL_ACTION).setPackage(activity.packageName)
        val flags = if (Build.VERSION.SDK_INT >= 31) {
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pending = PendingIntent.getBroadcast(activity, sessionId, resultIntent, flags)
        if (!updateInstallReceiverRegistered) {
            val filter = IntentFilter(UPDATE_INSTALL_ACTION)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                activity.registerReceiver(updateInstallReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                @Suppress("UnspecifiedRegisterReceiverFlag")
                activity.registerReceiver(updateInstallReceiver, filter)
            }
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
        val uri = androidx.core.content.FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", apk)
        activity.startActivity(
            Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "application/vnd.android.package-archive")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            },
        )
    }

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
        activity.submitTo(updateWorker, allowDuringFinish = true) {
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

    companion object {
        /** PackageInstaller 会话结果广播的私有 action */
        private const val UPDATE_INSTALL_ACTION = "com.joymanga.app.UPDATE_INSTALL_RESULT"
    }
}
