package com.joymanga.app

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.KeyEvent
import android.view.WindowManager
import android.view.inputmethod.InputMethodManager
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.RejectedExecutionException

internal fun Activity.submitTo(
    executor: ExecutorService,
    allowDuringFinish: Boolean = false,
    task: () -> Unit,
): Boolean {
    if (executor.isShutdown || (!allowDuringFinish && (isDestroyed || isFinishing))) return false
    return try {
        executor.execute(task)
        true
    } catch (_: RejectedExecutionException) {
        false
    }
}

class MainActivity : TauriActivity() {
    private var appWebView: WebView? = null
    internal val webView: WebView? get() = appWebView

    // 三个辅助类在基类 AttachBaseContext 之前初始化，构造方法内禁止触碰 Context 系统服务。
    private val networkTracker = AndroidNetworkTracker(this) { appWebView }
    private val storageHelper = AndroidStorageHelper(this) { appWebView }
    private val apkInstallManager = ApkInstallManager(this)

    private var visible = false
    private var volumeKeyPaging = false
    private var activeDownloads = 0
    private var hasDownloadStatus = false
    private val handler = Handler(Looper.getMainLooper())
    private var serviceUpdateScheduled = false
    private val updateDownloadServiceRunnable = Runnable { updateDownloadService() }
    private var downloadSpeed = ""
    private var downloadPagesCurrent = 0
    private var downloadPagesTotal = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        // 界面恒为浅色：在 super.onCreate 之前锁成浅色，避免深色模式切换时触发首帧闪烁或二次重建。
        AppCompatDelegate.setDefaultNightMode(AppCompatDelegate.MODE_NIGHT_NO)
        super.onCreate(savedInstanceState)
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        WindowCompat.setDecorFitsSystemWindows(window, true)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() { dispatchBack() }
        })
        storageHelper.scheduleStartupCleanup(isColdStart = savedInstanceState == null)
        apkInstallManager.scheduleStartupCleanup()
        // 冷启动时由「用 JoyManga 打开 CBZ」拉起：WebView 就绪后会自动导入
        storageHelper.handleViewIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        storageHelper.handleViewIntent(intent)
        // 点「安装包已下载完成」通知回来时 Activity 可能已经在前台（没有 onStart），
        // 这里补一次；待安装包取走后就不会再重复拉起安装器。
        apkInstallManager.installPendingUpdate(visible)
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        appWebView = webView
        storageHelper.onWebViewCreated()
        webView.addJavascriptInterface(AndroidBridge(), "JmAndroid")
        networkTracker.register()
    }

    override fun onStart() {
        super.onStart()
        visible = true
        AppState.activity = this
        AppState.foreground = true
        if (hasDownloadStatus) updateDownloadService()
        networkTracker.pushCurrentNetworkState()
        apkInstallManager.installPendingUpdate(visible)
    }

    override fun onStop() {
        visible = false
        if (AppState.activity === this) AppState.foreground = false
        // 不要在这里清 KEEP_SCREEN_ON 与 AppState.activity：原因见 DownloadService / 阅读常亮策略，
        // 后台权限由 AppState.foreground 约束，真正销毁在 onDestroy 处理。
        super.onStop()
    }

    override fun onDestroy() {
        handler.removeCallbacks(updateDownloadServiceRunnable)
        apkInstallManager.destroy()
        networkTracker.unregister()
        storageHelper.destroy()
        // 后台推送可能仍在队列里；清空引用后 evaluateJavascript 不会再碰已销毁的 WebView。
        appWebView = null
        val isCurrentActivity = AppState.activity === this
        if (isCurrentActivity) {
            AppState.activity = null
            AppState.foreground = false
        }
        // 非配置变更重建（如用户划掉卡片）导致 WebView 销毁后，前端无法再调 downloadStatus(0)，
        // 若放任服务继续运行会导致通知与 PARTIAL_WAKE_LOCK 永久续期；断点进度已落盘。
        if (!isChangingConfigurations && isCurrentActivity) {
            stopDownloadServiceAndNotification()
        }
        super.onDestroy()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean {
        if (keyCode == KeyEvent.KEYCODE_BACK) { dispatchBack(); return true }
        if (volumeKeyPaging && (keyCode == KeyEvent.KEYCODE_VOLUME_UP || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN)) {
            if (event?.repeatCount == 0) {
                val direction = if (keyCode == KeyEvent.KEYCODE_VOLUME_UP) "prev" else "next"
                appWebView?.evaluateJavascript(
                    "window.dispatchEvent(new CustomEvent('jm-volume-page', {detail:'$direction'}))",
                    null,
                )
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

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        storageHelper.onRequestPermissionsResult(requestCode, grantResults)
    }

    @Deprecated("Compatible document picker result")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        storageHelper.onActivityResult(requestCode, resultCode, data)
    }

    /** 供 [UpdateDownloadService] 调用：应用在前台时下载完成，直接拉起安装器。 */
    internal fun installDownloadedUpdate(apk: File) {
        apkInstallManager.installDownloadedUpdate(apk)
    }

    private fun stopDownloadServiceAndNotification() {
        try { stopService(Intent(this, DownloadService::class.java)) } catch (_: Exception) {}
        try { getSystemService(NotificationManager::class.java)?.cancel(DownloadService.ID) } catch (_: Exception) {}
    }

    private fun updateDownloadService() {
        handler.removeCallbacks(updateDownloadServiceRunnable)
        serviceUpdateScheduled = false
        if (activeDownloads <= 0) { stopDownloadServiceAndNotification(); return }
        val intent = Intent(this, DownloadService::class.java)
            .putExtra("active", activeDownloads)
            .putExtra("speed", downloadSpeed)
            .putExtra("current", downloadPagesCurrent)
            .putExtra("total", downloadPagesTotal)
        try {
            if (visible || AppState.downloadServiceRunning) {
                if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent) else startService(intent)
            } else {
                notifyRunningInBackground()
            }
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "下载前台服务启动失败", error)
            try { notifyRunningInBackground() } catch (_: Exception) {}
        }
    }

    /** 不能起前台服务时的退路：只更新通知。已运行的服务会接收更新，不会新建服务。 */
    private fun notifyRunningInBackground() {
        DownloadService.ensureChannel(this)
        getSystemService(NotificationManager::class.java).notify(
            DownloadService.ID,
            (if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, DownloadService.CHANNEL) else Notification.Builder(this))
                .setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle("正在下载 $activeDownloads 个章节")
                .setContentText(downloadSpeed).setOngoing(true).setOnlyAlertOnce(true)
                .setContentIntent(
                    android.app.PendingIntent.getActivity(
                        this,
                        0,
                        Intent(this, MainActivity::class.java),
                        android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE,
                    ),
                ).build(),
        )
    }

    /** 合并高频更新：按 750ms 节流重建通知，防止系统限流丢弃通知。 */
    private fun scheduleDownloadServiceUpdate() {
        if (serviceUpdateScheduled || isDestroyed || isFinishing) return
        serviceUpdateScheduled = true
        handler.postDelayed(updateDownloadServiceRunnable, 750)
    }

    /** 全部任务结束时发一条可自动消失的完成通知。 */
    private fun notifyDownloadsFinished() {
        try {
            DownloadService.ensureChannel(this)
            val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
            val pending = android.app.PendingIntent.getActivity(
                this,
                0,
                open,
                android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE,
            )
            val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, DownloadService.CHANNEL) else Notification.Builder(this)
            getSystemService(NotificationManager::class.java).notify(
                DownloadService.DONE_ID,
                builder.setSmallIcon(android.R.drawable.stat_sys_download_done)
                    .setContentTitle("下载完成").setContentText("点击查看下载与传输")
                    .setContentIntent(pending).setAutoCancel(true).setOnlyAlertOnce(true).build(),
            )
        } catch (error: Exception) { android.util.Log.w("JoyManga", "发送下载完成通知失败", error) }
    }

    /** 当前应用版本名（用于"每个版本最多提示一次通知权限"）。 */
    private fun appVersionName(): String = try {
        packageManager.getPackageInfo(packageName, 0).versionName ?: ""
    } catch (_: Exception) {
        ""
    }

    inner class AndroidBridge {
        @JavascriptInterface fun isMetered(): Boolean = networkTracker.isMetered()
        @JavascriptInterface fun isVpnActive(): Boolean = networkTracker.isVpnActive()
        @JavascriptInterface fun networkKey(): String = networkTracker.networkKey()
        @JavascriptInterface fun networkSnapshot(): String = networkTracker.networkSnapshot()

        @JavascriptInterface fun libraryPath(): String = storageHelper.libraryPath()
        @JavascriptInterface fun downloadsPath(): String = storageHelper.downloadsPath()
        @JavascriptInterface fun hasAllFilesAccess(): Boolean = storageHelper.hasAllFilesAccess()
        @JavascriptInterface fun supportsPublicStorage(): Boolean = storageHelper.supportsPublicStorage()
        @JavascriptInterface fun requestAllFilesAccess() = storageHelper.requestAllFilesAccess()
        @JavascriptInterface fun canWritePublicDownloads(): Boolean = storageHelper.canWritePublicDownloads()
        @JavascriptInterface fun notifyOpenFileReady() = storageHelper.notifyOpenFileReady()
        @JavascriptInterface fun pickDocuments(id: String, mode: String) = storageHelper.pickDocuments(id, mode)
        @JavascriptInterface fun discardImports(id: String) = storageHelper.discardImports(id)
        @JavascriptInterface fun exportDocuments(id: String, path: String) = storageHelper.exportDocuments(id, path)

        @JavascriptInterface fun downloadAndInstallApk(url: String, fileName: String) =
            apkInstallManager.downloadAndInstallApk(url, fileName)
        @JavascriptInterface fun updateDownloadState() = apkInstallManager.syncDownloadState()

        @JavascriptInterface fun exitApp() { runOnUiThread { moveTaskToBack(true) } }

        @JavascriptInterface fun downloadStatus(active: Int, speed: String, current: Int, total: Int) {
            runOnUiThread {
                if (isDestroyed || isFinishing) return@runOnUiThread
                hasDownloadStatus = true
                val wasActive = activeDownloads > 0
                activeDownloads = active.coerceAtLeast(0)
                downloadSpeed = speed.take(80)
                downloadPagesCurrent = current.coerceAtLeast(0)
                downloadPagesTotal = total.coerceAtLeast(0)
                val prefs = getSharedPreferences("joymanga_permissions", MODE_PRIVATE)
                val version = appVersionName()
                val askedVersion = prefs.getString("notifications_prompted_version", null)
                if (activeDownloads > 0 && Build.VERSION.SDK_INT >= 33 &&
                    checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
                    askedVersion != version
                ) {
                    prefs.edit().putString("notifications_prompted_version", version).apply()
                    requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQUEST_NOTIFICATION_PERMISSION)
                }
                scheduleDownloadServiceUpdate()
                if (activeDownloads > 0 && !wasActive) updateDownloadService()
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

        /** 阅读器沉浸式全屏：隐藏状态栏与导航栏，内容延伸到屏幕边缘。 */
        @JavascriptInterface fun setImmersiveMode(enabled: Boolean) {
            runOnUiThread {
                val controller = WindowCompat.getInsetsController(window, window.decorView)
                if (enabled) {
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

    companion object {
        private const val REQUEST_NOTIFICATION_PERMISSION = 101
    }
}
