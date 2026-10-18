package com.joymanga.app

import android.app.*
import android.content.Intent
import android.os.*
import android.content.pm.ServiceInfo

/**
 * 下载前台服务。
 *
 * 通知内容由前端通过 JmAndroid.downloadStatus 推来（活动章节数 / 进度 / 速度），
 * 这里只负责渲染：进行中显示进度条与"取消当前"，全部结束显示完成汇总。
 */
class DownloadService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private val handler = Handler(Looper.getMainLooper())
    private val renew = object : Runnable {
        override fun run() {
            wakeLock?.acquire(15 * 60 * 1000L)
            handler.postDelayed(this, 10 * 60 * 1000L)
        }
    }

    override fun onCreate() {
        super.onCreate()
        if (Build.VERSION.SDK_INT >= 26) {
            getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL, "漫画下载", NotificationManager.IMPORTANCE_LOW)
            )
        }
        wakeLock = (getSystemService(POWER_SERVICE) as PowerManager).newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK, "$packageName:downloads"
        ).apply { setReferenceCounted(false) }
        renew.run()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_CANCEL) {
            // 交给前端决定取消哪一个（前端持有任务列表），服务只负责转发
            sendToWeb("window.dispatchEvent(new Event('jm-download-cancel'))")
            return START_NOT_STICKY
        }
        val active = intent?.getIntExtra("active", 0) ?: 0
        if (active <= 0) { stopSelf(); return START_NOT_STICKY }

        val total = intent.getIntExtra("total", 0)
        val current = intent.getIntExtra("current", 0)
        val speed = intent.getStringExtra("speed").orEmpty()
        val notification = buildNotification(active, current, total, speed, done = false)
        if (Build.VERSION.SDK_INT >= 29) startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        else startForeground(ID, notification)
        return START_NOT_STICKY
    }

    private fun buildNotification(active: Int, current: Int, total: Int, speed: String, done: Boolean): Notification {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)

        if (done) {
            // 完成态：不带进度条，也不常驻
            return builder.setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle("下载完成")
                .setContentText(speed.ifBlank { "点击查看下载与传输" })
                .setContentIntent(pending)
                .setAutoCancel(true)
                .setOnlyAlertOnce(true)
                .build()
        }

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

    private fun sendToWeb(script: String) {
        try {
            val activity = AppState.activity ?: return
            activity.runOnUiThread { activity.webView?.evaluateJavascript(script, null) }
        } catch (_: Exception) {
        }
    }

    override fun onDestroy() {
        handler.removeCallbacks(renew)
        if (wakeLock?.isHeld == true) wakeLock?.release()
        wakeLock = null
        super.onDestroy()
    }
    override fun onTaskRemoved(rootIntent: Intent?) { stopSelf(); super.onTaskRemoved(rootIntent) }
    override fun onBind(intent: Intent?) = null
    companion object {
        const val CHANNEL = "joymanga_downloads"
        const val ID = 1201
        const val ACTION_CANCEL = "com.joymanga.app.DOWNLOAD_CANCEL"
    }
}

/** 极简的活动/WebView 引用，供服务把用户操作转发回前端。 */
object AppState {
    @Volatile var activity: MainActivity? = null
}
