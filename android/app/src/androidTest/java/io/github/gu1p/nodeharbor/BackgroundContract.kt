package io.github.gu1p.nodeharbor

import android.content.ComponentName
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.app.NotificationManager
import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.lifecycle.Lifecycle
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

class BackgroundContract {
    @Test fun closingTheActivityKeepsTheOptedInServiceAndNotificationPauseStopsIt() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val store = PrivateStore(context.noBackupFilesDir.resolve("nodeharbor"))
        val saved = store.load()
        check(saved.deviceId.isEmpty()) { "Use a dedicated unenrolled test installation for this contract" }
        InstrumentationRegistry.getInstrumentation().uiAutomation.grantRuntimePermission(context.packageName, android.Manifest.permission.POST_NOTIFICATIONS)
        val notifications = context.getSystemService(NotificationManager::class.java)
        fun await(message: String, condition: () -> Boolean) {
            val end = android.os.SystemClock.elapsedRealtime() + 10_000
            while (!condition() && android.os.SystemClock.elapsedRealtime() < end) Thread.sleep(50)
            assertTrue(message, condition())
        }
        try {
            ActivityScenario.launch(MainActivity::class.java).use { activity ->
                store.update { it.copy(policy = it.policy.copy(enabled = true, background = true), userStopped = false) }
                activity.onActivity { WorkerService.start(it) }
                await("A foreground notification must expose owner controls") { notifications.activeNotifications.any { it.id == 1 } }
                activity.moveToState(Lifecycle.State.CREATED)
                Thread.sleep(1500)
                val status = notifications.activeNotifications.single { it.id == 1 }.notification
                assertEquals(listOf("Pause", "Stop"), status.actions.map { it.title.toString() })
                status.actions.first { it.title == "Pause" }.actionIntent.send()
                await("Notification Pause must persist before the service exits") { store.load().userStopped && !store.load().policy.enabled }
                await("An explicitly paused idle service must stop") { notifications.activeNotifications.none { it.id == 1 } }
                store.update { it.copy(policy = it.policy.copy(background = true, startOnOpen = true, startAfterBoot = true)) }
                activity.moveToState(Lifecycle.State.RESUMED)
                val agent = (context.applicationContext as NodeHarborApplication).agent
                agent.startAutomatically(StartCause.Open)
                assertFalse("Service recreation must honor persisted owner stop", agent.recoverInBackground())
                assertTrue(store.load().userStopped)
                assertFalse(store.load().policy.enabled)
            }
        } finally {
            context.stopService(Intent(context, WorkerService::class.java))
            store.update { saved }
        }
    }
    @Test fun backgroundExecutionUsesAnUnexportedForegroundServiceThatSurvivesClosingTheTask() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val info = context.packageManager.getServiceInfo(ComponentName(context.packageName, "${context.packageName}.WorkerService"), PackageManager.ComponentInfoFlags.of(0))
        assertFalse(info.exported)
        assertEquals(0, info.flags and ServiceInfo.FLAG_STOP_WITH_TASK)
        if (Build.VERSION.SDK_INT >= 34) assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE, info.foregroundServiceType)
        val packageInfo = context.packageManager.getPackageInfo(context.packageName, PackageManager.PackageInfoFlags.of(PackageManager.GET_PERMISSIONS.toLong()))
        for (permission in listOf("FOREGROUND_SERVICE", "FOREGROUND_SERVICE_SPECIAL_USE", "WAKE_LOCK", "RECEIVE_BOOT_COMPLETED"))
            assertTrue(permission, packageInfo.requestedPermissions?.contains("android.permission.$permission") == true)
    }
}
