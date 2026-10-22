package com.joymanga.app

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

/**
 * 更新安装包下载前台服务。
 *
 * 安装包原先在 MainActivity 的工作线程里下载：应用切到后台后进程只是普通后台进程，
 * 国产 ROM 往往几秒内就把它冻结或回收，几十 MB 的包经常下到一半中断；Activity 被销毁时
 * `updateWorker.shutdownNow()` 还会直接把下载打断。改由前台服务下载后，通知栏一直显示
 * 进度，离开应用也能下完；进程被杀后再次点击更新，还能从断点续传。
 *
 * 下载完成后按应用是否可见分流：
 * - 前台：直接交给 [MainActivity.installDownloadedUpdate] 拉起系统安装器（与原来一致）；
 * - 后台：Android 10+ 不允许后台应用直接拉起界面，先记下待安装的包（内存 + 偏好设置，
 *   进程被杀也不丢）并发一条完成通知；用户点通知或回到应用时再安装。
 */
class UpdateDownloadService : Service() {
    private val worker = Executors.newSingleThreadExecutor()
    private val handler = Handler(Looper.getMainLooper())
    private var wakeLock: PowerManager.WakeLock? = null
    /** 上一次刷新进度通知的时间（elapsedRealtime）；通知刷新比前端事件稀疏，避免被系统限流 */
    private var lastNotifyAt = 0L

    private val renew = object : Runnable {
        override fun run() {
            wakeLock?.acquire(30 * 60 * 1000L)
            handler.postDelayed(this, 10 * 60 * 1000L)
        }
    }

    override fun onCreate() {
        super.onCreate()
        current = this
        ensureChannel(this)
        wakeLock = (getSystemService(Context.POWER_SERVICE) as PowerManager).newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK, "$packageName:update",
        ).apply { setReferenceCounted(false) }
        renew.run()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val url = intent?.getStringExtra(EXTRA_URL).orEmpty()
        val fileName = intent?.getStringExtra(EXTRA_FILE).orEmpty()
        // 无论走哪条分支都要先转前台：startForegroundService 之后 5 秒内不调用会直接崩溃
        if (running) {
            // 重复点击不重开下载：把当前进度补发一次，界面直接回到「下载中」
            startForegroundCompat(buildProgressNotification(lastPercent))
            if (url.isNotEmpty()) AppState.emitUpdate("progress", percent = lastPercent)
            return START_NOT_STICKY
        }
        lastPercent = 0
        startForegroundCompat(buildProgressNotification(0))
        if (url.isEmpty()) {
            stopForegroundCompat()
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        terminal = null
        // 新一轮下载开始：上一轮留下的待安装包作废，避免失败后误装旧版本
        pendingInstall = null
        clearPendingInstall(this)
        worker.execute { download(url, fileName) }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun download(url: String, fileName: String) {
        try {
            val target = updateTarget(fileName)
            val partial = partialFile(url, fileName)
            // 上次下载被系统杀掉时留下的半成品：先按已有长度请求剩余部分
            var resumeFrom = if (partial.exists()) partial.length() else 0L
            val connection = URL(url).openConnection() as HttpURLConnection
            try {
                connection.connectTimeout = 15_000
                connection.readTimeout = 30_000
                connection.instanceFollowRedirects = true
                // GitHub 对缺少 UA 的请求可能直接返回 403
                connection.setRequestProperty("User-Agent", "JoyManga-Android")
                if (resumeFrom > 0) connection.setRequestProperty("Range", "bytes=$resumeFrom-")
                connection.connect()
                if (resumeFrom > 0 && connection.responseCode == 416) {
                    // 服务器认为已经取到文件末尾：半成品就是完整包，直接进入安装
                    connection.disconnect()
                    require(partial.renameTo(target)) { "无法保存安装包" }
                    finishSuccess(target)
                    return
                }
                if (connection.responseCode == HttpURLConnection.HTTP_OK) {
                    // 服务器不支持断点续传（或忽略 Range）：从头写，不能接着追加
                    resumeFrom = 0
                } else {
                    require(connection.responseCode in 200..299) { "下载失败（HTTP ${connection.responseCode}）" }
                }
                val total = connection.contentLengthLong.let { if (it > 0) it + resumeFrom else it }
                connection.inputStream.use { input ->
                    FileOutputStream(partial, resumeFrom > 0).use { output ->
                        val buffer = ByteArray(128 * 1024)
                        var copied = resumeFrom
                        var stepMark = copied
                        var lastReported = -1
                        val step = (total / 100).coerceAtLeast(64 * 1024)
                        while (true) {
                            val count = input.read(buffer)
                            if (count < 0) break
                            output.write(buffer, 0, count)
                            copied += count
                            if (total > 0 && copied - stepMark >= step) {
                                stepMark = copied
                                val percent = (copied * 100 / total).toInt().coerceIn(0, 99)
                                if (percent != lastReported) {
                                    lastReported = percent
                                    report(percent)
                                }
                            }
                        }
                        output.fd.sync()
                    }
                }
            } finally {
                connection.disconnect()
            }
            require(partial.length() > 0) { "下载的安装包为空" }
            require(partial.renameTo(target)) { "无法保存安装包" }
            finishSuccess(target)
        } catch (error: Exception) {
            val message = error.message ?: "下载更新失败"
            terminal = "error" to message
            AppState.emitUpdate("error", message = message)
            notifyFailure(message)
        } finally {
            // 失败时保留半成品：下次点击更新可以从断点继续
            running = false
            // 下载期间服务可能被系统销毁重建（running 为 true 时新实例只负责维持
            // 前台状态），结束时要停的是当前存活的实例，而不是发起下载的那个。
            val alive = current ?: this
            alive.stopForegroundCompat()
            alive.stopSelf()
        }
    }

    /** 进度回传前端（每 1%），通知栏按 500ms 节流刷新。 */
    private fun report(percent: Int) {
        lastPercent = percent
        AppState.emitUpdate("progress", percent = percent)
        val now = SystemClock.elapsedRealtime()
        if (now - lastNotifyAt < 500) return
        lastNotifyAt = now
        try {
            notificationManager().notify(ID, buildProgressNotification(percent))
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "刷新更新下载通知失败", error)
        }
    }

    private fun finishSuccess(apk: File) {
        lastPercent = 100
        AppState.emitUpdate("done", percent = 100)
        // 应用在前台：直接拉起安装器，保持原来的「点更新→确认安装」一步到位。
        // 只能看 foreground：后台时 activity 依然存在，但从后台 startActivity 在
        // Android 10+ 会被系统直接忽略，安装器永远弹不出来。
        val activity = AppState.activity
        if (activity != null && AppState.foreground) {
            terminal = "installer" to "请在系统弹窗中确认安装"
            activity.installDownloadedUpdate(apk)
            return
        }
        // 后台：记下来（内存 + 偏好设置，进程被杀也不丢），交给通知
        terminal = "installer" to "安装包已下载完成，请继续安装"
        pendingInstall = apk
        savePendingInstall(apk)
        notifyReady()
    }

    /** 安装包落盘路径；顺带清掉 1 小时前的旧包（系统安装器可能还在读，不能删新的） */
    private fun updateTarget(fileName: String): File {
        val name = safeFileName(fileName).ifBlank { "update.apk" }.let {
            if (it.endsWith(".apk", ignoreCase = true)) it else "$it.apk"
        }
        val dir = File(cacheDir, "updates").apply { mkdirs() }
        dir.listFiles()?.filter { it.lastModified() < System.currentTimeMillis() - 60 * 60 * 1000 }?.forEach { it.delete() }
        return File(dir, name)
    }

    /**
     * 半成品路径。按下载地址的哈希区分：换了版本的安装包绝不能接着旧文件续传，
     * 否则拼出的是两个 APK 的混合体，装上去只会校验失败。
     */
    private fun partialFile(url: String, fileName: String): File {
        val name = safeFileName(fileName).ifBlank { "update.apk" }.let {
            if (it.endsWith(".apk", ignoreCase = true)) it else "$it.apk"
        }
        val tag = Integer.toHexString(url.hashCode()).padStart(8, '0')
        return File(File(cacheDir, "updates"), ".part-$tag-$name")
    }

    private fun buildProgressNotification(percent: Int): Notification {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(
            this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        return builder.setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("正在下载更新")
            .setContentText("$percent%")
            .setContentIntent(pending)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setProgress(100, percent, false)
            .build()
    }

    private fun notifyReady() {
        try {
            val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
            val pending = PendingIntent.getActivity(
                this, 2, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
            notificationManager().notify(
                READY_ID,
                builder.setSmallIcon(android.R.drawable.stat_sys_download_done)
                    .setContentTitle("安装包已下载完成")
                    .setContentText("点击继续安装")
                    .setContentIntent(pending)
                    .setAutoCancel(true)
                    .setOnlyAlertOnce(true)
                    .build(),
            )
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "发送更新完成通知失败", error)
        }
    }

    private fun notifyFailure(message: String) {
        try {
            val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
            val pending = PendingIntent.getActivity(
                this, 3, open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
            notificationManager().notify(
                FAILED_ID,
                builder.setSmallIcon(android.R.drawable.stat_notify_error)
                    .setContentTitle("更新下载失败")
                    .setContentText(message.take(80))
                    .setContentIntent(pending)
                    .setAutoCancel(true)
                    .setOnlyAlertOnce(true)
                    .build(),
            )
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "发送更新失败通知失败", error)
        }
    }

    private fun savePendingInstall(apk: File) {
        try {
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(KEY_PENDING_APK, apk.absolutePath).apply()
        } catch (error: Exception) {
            android.util.Log.w("JoyManga", "记录待安装更新包失败", error)
        }
    }

    private fun notificationManager() = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

    private fun startForegroundCompat(notification: Notification) {
        if (Build.VERSION.SDK_INT >= 29) startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        else startForeground(ID, notification)
    }

    private fun stopForegroundCompat() {
        stopForeground(STOP_FOREGROUND_REMOVE)
    }

    override fun onDestroy() {
        if (current === this) current = null
        handler.removeCallbacks(renew)
        if (wakeLock?.isHeld == true) wakeLock?.release()
        wakeLock = null
        // 注意：不要在这里 shutdownNow，也不要在这里复位 running。
        // 服务可能在下到一半时被系统销毁，而下载线程仍在同一进程里继续跑完，
        // 它自己的 finally 会复位 running；提前打断只会留下半截文件，
        // 提前复位则允许再开第二路下载，两路同时写同一个半成品必然写坏。
        worker.shutdown()
        super.onDestroy()
    }

    companion object {
        const val EXTRA_URL = "url"
        const val EXTRA_FILE = "file_name"
        const val CHANNEL = "joymanga_updates"
        const val ID = 1203
        const val READY_ID = 1204
        const val FAILED_ID = 1205
        private const val PREFS = "joymanga_updates"
        private const val KEY_PENDING_APK = "pending_apk"

        /**
         * 是否有安装包正在下载；界面重建后也据此拦住重复点击，并补发当前进度。
         * 放在伴生对象里而不是 Service 实例上：服务被系统销毁重建时实例是新的，
         * 实例字段会丢掉「已有一轮下载在跑」的判断。
         */
        @Volatile private var running = false
        /** 最近一次报告的百分比，供界面回到前台时回填 */
        @Volatile private var lastPercent = 0
        /**
         * 最近一次下载的结局（state 到 message）。界面回到前台时用它补发，
         * 否则后台下载完成后回来，界面会一直停在「下载中」。
         */
        @Volatile private var terminal: Pair<String, String>? = null
        /** 后台下载完成、等待用户确认安装的安装包 */
        @Volatile private var pendingInstall: File? = null
        /** 当前存活的服务实例；下载结束时用它停掉前台通知 */
        @Volatile private var current: UpdateDownloadService? = null

        fun isRunning(): Boolean = running

        fun progress(): Int = lastPercent

        /** 取走最近一次下载的结局（只补发一次，避免每次回到前台都重现旧提示） */
        fun takeOutcome(): Pair<String, String>? {
            val outcome = terminal
            terminal = null
            return outcome
        }

        /**
         * 取出待安装的安装包并清空记录，避免回到前台后重复拉起安装器。
         * 内存里没有时回落到偏好设置：下载完成与下次启动之间进程可能已被系统杀掉。
         */
        fun takePendingInstall(context: Context): File? {
            val memory = pendingInstall
            pendingInstall = null
            val stored = try {
                context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_PENDING_APK, null)
            } catch (_: Exception) {
                null
            }
            clearPendingInstall(context)
            val path = memory?.absolutePath ?: stored ?: return null
            return File(path).takeIf { it.exists() }
        }

        fun clearPendingInstall(context: Context) {
            try {
                context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().remove(KEY_PENDING_APK).apply()
            } catch (_: Exception) {
            }
        }

        fun clearReadyNotification(context: Context) {
            try {
                context.getSystemService(NotificationManager::class.java).cancel(READY_ID)
            } catch (_: Exception) {
            }
        }

        fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < 26) return
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                android.app.NotificationChannel(CHANNEL, "应用更新", NotificationManager.IMPORTANCE_LOW),
            )
        }
    }
}
