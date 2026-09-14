package io.github.gu1p.nodeharbor

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.Base64

class AppUpdateTest {
    @Test fun cleanDevelopmentBuildsStillRequireManualUpdateChecks() {
        assertFalse(automaticReleaseUpdates("0.1.0-main-port-development", false))
        assertFalse(automaticReleaseUpdates("0.1.73", true))
        assertTrue(automaticReleaseUpdates("0.1.73", false))
    }
    private fun manifest(version: String = "0.1.73", url: String = "https://updates.example/nodeharbor.apk") = JSONObject()
        .put("version", version).put("commit", "a".repeat(40))
        .put("platforms", JSONObject().put("android-aarch64-apk", JSONObject().put("url", url)
            .put("signature", Base64.getEncoder().encodeToString("untrusted comment: test\nsignature".toByteArray())))).toString()

    @Test fun onlyNewerAndroidPackagesWithHttpsAndBoundedSignaturesAreCandidates() {
        assertEquals("0.1.73", parseAppUpdate(manifest(), "0.1.72")?.version)
        assertNull(parseAppUpdate(manifest(), "0.1.73"))
        assertNull(parseAppUpdate(manifest(), "0.1.74"))
        for (url in listOf("http://updates.example/a.apk", "file:///tmp/a.apk", "https://user:password@updates.example/a.apk"))
            assertThrows(IllegalArgumentException::class.java) { parseAppUpdate(manifest(url = url), "0.1.72") }
        assertThrows(IllegalArgumentException::class.java) { parseAppUpdate(manifest("0.1.73-beta"), "0.1.72") }
        assertThrows(IllegalArgumentException::class.java) { parseAppUpdate(" ".repeat(1024 * 1024 + 1), "0.1.72") }
    }

    @Test fun appUpdatePreferencesAndVerifiedInstallationIntentSurviveRestart() {
        val original = StoredState(policy = PhonePolicy(enabled = true), installedVersion = 10)
        val encoded = JSONObject(original.encode()).put("automaticUpdates", false)
            .put("applicationUpdatePending", true).put("updateInstallVersion", 11)
        val restored = StoredState.decode(encoded.toString(), 10)
        assertFalse(JSONObject(restored.encode()).getBoolean("automaticUpdates"))
        assertTrue(JSONObject(restored.encode()).getBoolean("applicationUpdatePending"))
        val updated = StoredState.decode(encoded.toString(), 11)
        assertTrue(updated.policy.enabled)
        assertFalse(updated.userStopped)
        assertFalse(JSONObject(updated.encode()).getBoolean("applicationUpdatePending"))
        val paused = encoded.put("userStopped", true).put("shared", original.policy.copy(enabled = false).sharedJson())
        assertFalse(StoredState.decode(paused.toString(), 11).policy.enabled)
        assertTrue(StoredState.decode(paused.toString(), 11).userStopped)
        assertFalse(StoredState.decode(encoded.toString(), 12).policy.enabled)
    }
}
