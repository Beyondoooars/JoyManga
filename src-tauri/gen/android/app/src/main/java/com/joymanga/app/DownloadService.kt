package com.joymanga.app

import android.app.*
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.*
import android.util.Log

/**
 * 下载前台服务。
 *
 * 通知内容由前端通过 JmAndroid.downloadStatus 推来（活动章节数 / 进度 / 速度），
 * 这里只负责渲染：进行中显示进度条与"取消当前"，全部结束显示完成汇总。
 */
class DownloadService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private var foregroundStarted = false
    private val handler = Handler(Looper.getMainLooper())
    private val renew = object : Runnable {
        override fun run() {
            try {
                wakeLock?.acquire(15 * 60 * 1000L)
            } catch (error: Exception) {
                Log.w("JoyManga", "续期下载 WakeLock 失败", error)
            }
            handler.postDelayed(this, 10 * 60 * 1000L)
        }
    }

    override fun onCreate() {
        super.onCreate()
        AppState.downloadServiceRunning = true
        ensureChannel(this)
        wakeLock = (getSystemService(POWER_SERVICE) as PowerManager).newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK, "$packageName:downloads"
        ).apply { setReferenceCounted(false) }
        renew.run()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_CANCEL) {
            // 若通过 startForegroundService 冷拉起，必须先转前台再决定去留，避免 5 秒超时崩溃
            if (!foregroundStarted && !startForegroundSafe(buildIdleNotification("正在取消下载"))) {
                return START_NOT_STICKY
            }
            // 交给前端决定取消哪一个（前端持有任务列表）；若前端已销毁，主动清理服务与唤醒锁
            val delivered = sendToWeb("window.dispatchEvent(new Event('jm-download-cancel'))")
            if (!delivered) {
                stopGracefully(startId)
            }
            return START_NOT_STICKY
        }
        val active = intent?.getIntExtra("active", 0) ?: 0
        if (intent == null || active <= 0) {
            if (!foregroundStarted) {
                startForegroundSafe(buildIdleNotification("下载已完成"))
            }
            stopGracefully(startId)
            return START_NOT_STICKY
        }

        val total = intent.getIntExtra("total", 0)
        val current = intent.getIntExtra("current", 0)
        val speed = intent.getStringExtra("speed").orEmpty()
        val notification = buildNotification(active, current, total, speed)
        if (!startForegroundSafe(notification)) {
            return START_NOT_STICKY
        }
        return START_NOT_STICKY
    }

    private fun startForegroundSafe(notification: Notification): Boolean {
        return try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(ID, notification)
            }
            foregroundStarted = true
            true
        } catch (error: Exception) {
            Log.w("JoyManga", "启动下载前台服务失败", error)
            stopGracefully()
            false
        }
    }

    private fun stopForegroundCompat() {
        if (!foregroundStarted) return
        foregroundStarted = false
        stopForeground(STOP_FOREGROUND_REMOVE)
    }

    private fun releaseWakeLockSafely() {
        val lock = wakeLock ?: return
        wakeLock = null
        try {
            if (lock.isHeld) lock.release()
        } catch (error: RuntimeException) {
            Log.w("JoyManga", "释放下载 WakeLock 失败", error)
        }
    }

    private fun stopGracefully(startId: Int? = null) {
        handler.removeCallbacks(renew)
        releaseWakeLockSafely()
        AppState.downloadServiceRunning = false
        stopForegroundCompat()
        if (startId != null) stopSelf(startId) else stopSelf()
    }

    private fun buildIdleNotification(title: String): Notification {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        return builder.setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(title)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(pending)
            .build()
    }

    private fun buildNotification(active: Int, current: Int, total: Int, speed: String): Notification {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)

        builder.setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("正在下载 $active 个章节")
            .setContentText(speed.ifBlank { "点击查看下载与传输" })
            .setContentIntent(pending)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(
                0,
                "取消当前",
                PendingIntent.getService(
                    this,
                    1,
                    Intent(this, DownloadService::class.java).setAction(ACTION_CANCEL),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            )

        // 有章节级进度就画进度条，否则用不确定进度
        if (total > 0) {
            val percent = (current * 100 / total).coerceIn(0, 100)
            builder.setProgress(100, percent, false)
                .setSubText("$current / $total 页")
        } else {
            builder.setProgress(0, 0, true)
        }
        return builder.build()
    }

    /**
     * 把「取消当前」转发给前端。
     *
     * 之前 Activity 在 onStop 时就被清掉，应用一切到后台，通知栏的「取消当前」
     * 点了什么也不会发生；现在只在真正销毁（onDestroy）时清空，后台仍能转发。
     */
    private fun sendToWeb(script: String) = AppState.emit(script)

    override fun onTimeout(startId: Int) {
        Log.w("JoyManga", "下载前台服务达到系统时长限制，安全释放并停止")
        stopGracefully(startId)
    }

    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w("JoyManga", "下载前台服务达到系统时长限制（fgsType=$fgsType），安全释放并停止")
        stopGracefully(startId)
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // 最近任务划掉界面导致后续无法再通过前端推进/结束前台通知时，及时收口唤醒锁；
        // 断点进度已在 Rust 下载队列持久化，下次冷启动会自动恢复。
        stopGracefully()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        AppState.downloadServiceRunning = false
        handler.removeCallbacks(renew)
        releaseWakeLockSafely()
        stopForegroundCompat()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?) = null
    companion object {
        const val CHANNEL = "joymanga_downloads"
        const val ID = 1201
        const val DONE_ID = 1202
        const val ACTION_CANCEL = "com.joymanga.app.DOWNLOAD_CANCEL"

        /** 完成通知不经过服务（服务已停止），需要单独确保渠道存在。 */
        fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < 26) return
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL, "漫画下载", NotificationManager.IMPORTANCE_LOW)
            )
        }
    }
}

/**
 * 极简的活动/WebView 引用，供服务把用户操作转发回前端。
 *
 * `activity` 只在销毁时清空，切到后台（onStop）时保留：下载服务在后台仍要能把
 * 「取消当前」转发给前端，否则通知上的按钮点了没有任何反应。能否拉起界面由
 * [foreground] 判断——Android 10+ 禁止后台应用启动 Activity。
 */
object AppState {
    @Volatile var activity: MainActivity? = null
    @Volatile var foreground = false
    /** 漫画下载前台服务是否在运行；运行中时再次启动不受「后台不能起前台服务」限制 */
    @Volatile var downloadServiceRunning = false
    /** 待前端通知就绪的外部打开 URI 队列：放在全局单例里避免 Activity 重建丢失 */
    val pendingViewUris = ArrayDeque<Uri>()
    private val activeStagingDirs = HashSet<String>()

    fun markStagingActive(dirName: String) {
        synchronized(activeStagingDirs) { activeStagingDirs.add(dirName) }
    }

    fun unmarkStagingActive(dirName: String) {
        synchronized(activeStagingDirs) { activeStagingDirs.remove(dirName) }
    }

    fun isStagingActive(dirName: String): Boolean {
        synchronized(activeStagingDirs) { return activeStagingDirs.contains(dirName) }
    }

    /** @return 事件是否已投递给 WebView；Activity 已销毁或正在结束时返回 false */
    fun emit(script: String): Boolean {
        val activity = activity ?: return false
        if (activity.isDestroyed || activity.isFinishing) return false
        return try {
            activity.runOnUiThread {
                if (activity.isDestroyed || activity.isFinishing) return@runOnUiThread
                try { activity.webView?.evaluateJavascript(script, null) } catch (_: Exception) {}
            }
            true
        } catch (_: Exception) {
            false
        }
    }

    /** 更新流程进度/结果回传前端；事件名与 `src/utils/androidStorage.ts` 的订阅保持一致。 */
    fun emitUpdate(state: String, percent: Int = -1, message: String = "") {
        val payload = org.json.JSONObject().put("state", state)
        if (percent >= 0) payload.put("percent", percent)
        if (message.isNotEmpty()) payload.put("message", message)
        emit("window.dispatchEvent(new CustomEvent('jm-update-progress', {detail:$payload}))")
    }
}
