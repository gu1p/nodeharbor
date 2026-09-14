package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.UUID

class AppUpdateContract {
    @Test fun updateMaintenancePreservesOwnerChoicesAndRejectsOldCancellation() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = context.cacheDir.resolve("update-owner-${UUID.randomUUID()}")
        val store = PrivateStore(directory)
        val policy = PhonePolicy(enabled = true, background = true)
        store.update { it.copy(policy = policy) }
        val supervisor = PhoneSupervisor(context, store, { error("No controller request is needed for a stopped worker") },
            { _, _, _, _, _ -> }, {}, {})
        try {
            supervisor.beginApplicationUpdate(41)
            assertEquals(UpdateGate.Ready, supervisor.applicationUpdateGate(41))
            supervisor.cancelApplicationUpdate(40)
            org.junit.Assert.assertTrue(store.load().applicationUpdatePending)
            supervisor.cancelApplicationUpdate(41)
            assertEquals(policy, store.load().policy)
            org.junit.Assert.assertFalse(store.load().applicationUpdatePending)
            supervisor.beginApplicationUpdate(42)
            store.update { it.copy(userStopped = true, policy = it.policy.copy(enabled = false)) }
            supervisor.cancelApplicationUpdate(42)
            org.junit.Assert.assertTrue(store.load().userStopped)
            org.junit.Assert.assertFalse(store.load().policy.enabled)
        } finally { supervisor.close(); directory.deleteRecursively() }
    }

    @Test fun installationUsesAndroidPermissionAndAnUnexportedResultReceiver() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val manager = context.packageManager
        val info = manager.getPackageInfo(context.packageName, android.content.pm.PackageManager.PackageInfoFlags.of(
            android.content.pm.PackageManager.GET_PERMISSIONS.toLong()))
        org.junit.Assert.assertTrue("Self-updates must use Android's installation permission", info.requestedPermissions.orEmpty()
            .contains(android.Manifest.permission.REQUEST_INSTALL_PACKAGES))
        val receiver = manager.getReceiverInfo(android.content.ComponentName(context.packageName,
            context.packageName + ".UpdateInstallReceiver"), android.content.pm.PackageManager.ComponentInfoFlags.of(0))
        org.junit.Assert.assertFalse("Other apps cannot forge installer outcomes", receiver.exported)
    }

    @Test fun nativeVerificationRejectsUntrustedPackagesUsingTheEmbeddedReleaseKey() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = context.cacheDir.resolve("unsigned-update-${UUID.randomUUID()}.apk")
        try {
            file.writeText("This is not a signed application update")
            val request = JSONObject().put("operation", "verifyUpdate").put("path", file.absolutePath)
                .put("signature", "untrusted comment: invalid\ninvalid")
            assertEquals("false", NativeRules.request(request.toString()))
        } finally { file.delete() }
    }
}
