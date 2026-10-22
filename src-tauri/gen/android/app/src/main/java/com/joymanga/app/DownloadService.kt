package com.joymanga.app

import android.app.*
import android.content.Context
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
        AppState.downloadServiceRunning = true
        ensureChannel(this)
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

        // 走到这里 active > 0，说明 intent 非空；用局部非空引用避免可空接收者报错。
        val extras = intent ?: run { stopSelf(); return START_NOT_STICKY }
        val total = extras.getIntExtra("total", 0)
        val current = extras.getIntExtra("current", 0)
        val speed = extras.getStringExtra("speed").orEmpty()
        val notification = buildNotification(active, current, total, speed)
        if (Build.VERSION.SDK_INT >= 29) startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        else startForeground(ID, notification)
        return START_NOT_STICKY
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

    override fun onDestroy() {
        AppState.downloadServiceRunning = false
        handler.removeCallbacks(renew)
        if (wakeLock?.isHeld == true) wakeLock?.release()
        wakeLock = null
        super.onDestroy()
    }
    // 从最近任务划掉界面时不要停服务：下载在 Rust 侧继续执行，停掉前台服务会让
    // 进程更容易被系统回收。任务真正结束时前端把 active 置 0，服务自行停止。
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

    /** @return 事件是否已投递给 WebView；Activity 已销毁时返回 false */
    fun emit(script: String): Boolean {
        val activity = activity ?: return false
        return try {
            activity.runOnUiThread {
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
