package io.github.gu1p.nodeharbor

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class EnrollmentRecoveryTest {
    private val id = "9511182e-9c48-4d20-a15b-1da8bb441386"
    private val credential = JSONObject().put("deviceId", id).put("address", "https://fleet.example.com")
        .put("token", "a".repeat(64)).toString()

    @Test fun interruptedEnrollmentRecoversItsExistingIdentityWithSharingOff() {
        val saved = recoverEnrollment(StoredState(policy = PhonePolicy(cpus = 3)), credential)
        assertEquals(id, saved.deviceId)
        assertEquals("https://fleet.example.com", saved.controllerUrl)
        assertEquals(3, saved.policy.cpus)
        assertFalse(saved.policy.enabled)
    }
    @Test fun existingIdentityAndCredentialOriginMustAgree() {
        val saved = StoredState(deviceId = id, controllerUrl = "https://fleet.example.com")
        assertEquals(saved, recoverEnrollment(saved, credential))
        assertThrows(IllegalStateException::class.java) { recoverEnrollment(saved.copy(controllerUrl = "https://other.example.com"), credential) }
        assertThrows(IllegalStateException::class.java) { recoverEnrollment(saved, null) }
    }
}
