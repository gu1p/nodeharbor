package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class StoredStateTest {
    @Test fun restartPreservesAnOwnerResetAndTheOriginalDrainDeadline() {
        val original = StoredState(resetRequest = "00000000-0000-4000-8000-000000000002", drainDeadlineMillis = 123456)
        val restored = StoredState.decode(original.encode(), original.installedVersion)
        assertEquals(original.resetRequest, restored.resetRequest)
        assertEquals(original.drainDeadlineMillis, restored.drainDeadlineMillis)
    }
    @Test fun updatesPreserveEnrollmentAndLimitsButPauseSharing() {
        val original = StoredState(policy = PhonePolicy(enabled = true, memoryMib = 3072).continuous(),
            deviceId = "00000000-0000-4000-8000-000000000001", controllerUrl = "https://fleet.example", installedVersion = 10)
        val updated = StoredState.decode(original.encode(), 11)
        assertEquals("00000000-0000-4000-8000-000000000001", updated.deviceId)
        assertEquals(3072, updated.policy.memoryMib)
        assertTrue(updated.policy.background)
        assertFalse(updated.policy.enabled)
        assertTrue(updated.userStopped)
    }

    @Test fun futureOrDamagedStateCannotSilentlyReenableWork() {
        for (json in listOf("broken", "{\"formatVersion\":999}")) {
            assertThrows(IllegalArgumentException::class.java) { StoredState.decode(json, 1) }
        }
    }

    @Test fun savedStateNeverContainsTheEnrollmentToken() {
        val json = StoredState(policy = PhonePolicy().continuous()).encode()
        assertFalse(json.contains("token", ignoreCase = true))
        assertFalse(json.contains("credential", ignoreCase = true))
    }
}
