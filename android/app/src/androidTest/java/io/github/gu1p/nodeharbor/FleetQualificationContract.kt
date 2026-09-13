package io.github.gu1p.nodeharbor

import android.Manifest
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.util.Base64
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.security.MessageDigest

private fun vpnFingerprint(context: Context): String {
    val network = context.getSystemService(ConnectivityManager::class.java)
    val vpn = network.allNetworks.filter { network.getNetworkCapabilities(it)?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true }
        .map { connection ->
            val links = checkNotNull(network.getLinkProperties(connection)) { "VPN link information is unavailable" }
            listOf(links.interfaceName ?: "", links.routes.map { it.toString() }.sorted().joinToString(";"),
                links.dnsServers.map { it.hostAddress ?: "" }.sorted().joinToString(";"),
                links.linkAddresses.map { it.toString() }.sorted().joinToString(";")).joinToString("|")
        }.sorted().joinToString("\n")
    return MessageDigest.getInstance("SHA-256").digest(vpn.toByteArray()).joinToString("") { "%02x".format(it) }
}

class VpnSnapshotContract {
    @Test fun recordTheExistingVpnWithoutChangingItsSettings() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        instrumentation.sendStatus(2, Bundle().apply { putString("vpnFingerprint", vpnFingerprint(instrumentation.targetContext)) })
    }
}

class FleetSetupContract {
    @Test fun prepareAndQualifyThroughTheSameOwnerControlsAsTheNativeApp() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val encoded = InstrumentationRegistry.getArguments().getString("qualification")
        check(!encoded.isNullOrEmpty()) { "This large contract requires a dedicated authorized fleet qualification run" }
        val config = JSONObject(Base64.decode(encoded, Base64.DEFAULT).toString(Charsets.UTF_8))
        check(config.getBoolean("dedicatedDevice"))
        val context = instrumentation.targetContext
        val store = PrivateStore(context.noBackupFilesDir.resolve("nodeharbor"))
        val app = context.applicationContext as NodeHarborApplication
        val before = vpnFingerprint(context)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        if (Build.VERSION.SDK_INT >= 37) instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.ACCESS_LOCAL_NETWORK)
        val address = controllerAddress(config.getString("controller"))
        val saved = store.load()
        check(saved.deviceId.isEmpty() || saved.controllerUrl == address) { "The dedicated phone belongs to another fleet; its enrollment was preserved" }
        fun await(message: String, seconds: Long, done: () -> Boolean) {
            val deadline = SystemClock.elapsedRealtime() + seconds * 1000
            while (!done() && SystemClock.elapsedRealtime() < deadline) {
                check(app.agent.state.value.error.isEmpty()) { app.agent.state.value.error }
                Thread.sleep(1000)
            }
            assertTrue(message, done())
        }
        ActivityScenario.launch(MainActivity::class.java).use { activity ->
            if (saved.deviceId.isEmpty()) {
                app.agent.enroll(address, config.getString("enrollmentCode"))
                await("The dedicated phone must enroll through the authenticated app flow", 60) { store.load().deviceId.isNotEmpty() }
            }
            app.agent.action(UiAction.SavePolicy(PhonePolicy(drainSeconds = 60).continuous()))
            await("The dedicated owner policy must be saved", 30) { store.load().policy.background && store.load().policy.preventSleep && store.load().policy.drainSeconds == 60 }
            app.agent.action(UiAction.Resume)
            await("The owner must explicitly enable sharing", 30) { store.load().policy.enabled }
            activity.moveToState(Lifecycle.State.CREATED)
            await("The actual worker must complete preparation and normal fleet CI qualification", 4200) {
                app.agent.state.value.workerInstalled && app.agent.state.value.eligibleCi
            }
            assertEquals("The existing phone VPN must remain unchanged", before, vpnFingerprint(context))
            instrumentation.sendStatus(2, Bundle().apply {
                putString("deviceId", store.load().deviceId)
                putString("ciQualified", "true")
            })
        }
    }
}

class FleetStopContract {
    @Test fun theOwnerStopsTheRealWorkerAndItsForegroundService() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val agent = (context.applicationContext as NodeHarborApplication).agent
        val deadline = SystemClock.elapsedRealtime() + 120_000
        val notifications = context.getSystemService(android.app.NotificationManager::class.java)
        val current = notifications.activeNotifications.firstOrNull { it.id == 1 }
        if (current == null) agent.stopOwner()
        else current.notification.actions.single { it.title == "Pause" }.actionIntent.send()
        while ((!agent.supervisor.confirmedIdle || notifications.activeNotifications.any { it.id == 1 }) && SystemClock.elapsedRealtime() < deadline) Thread.sleep(1000)
        assertTrue("Android must confirm the worker stopped", agent.supervisor.confirmedIdle)
        assertFalse("The foreground worker must stop after the bounded drain", notifications.activeNotifications.any { it.id == 1 })
        assertFalse(PrivateStore(context.noBackupFilesDir.resolve("nodeharbor")).load().policy.enabled)
    }
}
