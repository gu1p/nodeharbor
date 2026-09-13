package io.github.gu1p.nodeharbor

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/** Public foreground-service APIs, with explicit owner controls in the notification. */
class WorkerService : Service() {
    private val agent get() = (application as NodeHarborApplication).agent
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Worker status", NotificationManager.IMPORTANCE_LOW))
        val notification = notification("Checking your sharing rules")
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTIFICATION, notification)
        agent.supervisor.attach { Handler(Looper.getMainLooper()).post { stopSelf() } }
        scope.launch { agent.state.collect { getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(it.reason)) } }
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action in listOf(PAUSE, STOP)) agent.stopOwner()
        return if (agent.recoverInBackground()) START_STICKY else START_NOT_STICKY
    }
    override fun onBind(intent: Intent): IBinder? = null
    override fun onTaskRemoved(rootIntent: Intent?) { agent.visibility(false); super.onTaskRemoved(rootIntent) }
    override fun onTimeout(startId: Int, fgsType: Int) { agent.stopOwner(); stopSelf() }
    override fun onDestroy() {
        scope.cancel()
        agent.supervisor.close()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
    private fun notification(reason: String): Notification {
        val immutable = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), immutable)
        fun control(action: String, code: Int) = PendingIntent.getService(this, code, Intent(this, WorkerService::class.java).setAction(action), immutable)
        return Notification.Builder(this, CHANNEL).setSmallIcon(R.drawable.ic_nodeharbor)
            .setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentTitle("NodeHarbor worker").setContentText(reason).setStyle(Notification.BigTextStyle().bigText(reason))
            .setContentIntent(open).setOngoing(true).setOnlyAlertOnce(true).setCategory(Notification.CATEGORY_SERVICE)
            .addAction(Notification.Action.Builder(null, "Pause", control(PAUSE, 1)).build())
            .addAction(Notification.Action.Builder(null, "Stop", control(STOP, 2)).build()).build()
    }
    companion object {
        private const val CHANNEL = "nodeharbor-worker"
        private const val NOTIFICATION = 1
        private const val PAUSE = "io.github.gu1p.nodeharbor.PAUSE"
        private const val STOP = "io.github.gu1p.nodeharbor.STOP"
        fun start(context: Context) { context.startForegroundService(Intent(context, WorkerService::class.java)) }
    }
}

class WorkerBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action !in listOf(Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED)) return
        val pending = goAsync()
        Thread({
            try {
                val agent = (context.applicationContext as NodeHarborApplication).agent
                if (intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) agent.stopOwner()
                else agent.startAutomatically(StartCause.Boot)
            } finally { pending.finish() }
        }, "nodeharbor-boot-policy").start()
    }
}
