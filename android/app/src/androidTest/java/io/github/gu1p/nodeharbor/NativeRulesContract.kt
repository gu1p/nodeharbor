package io.github.gu1p.nodeharbor

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class NativeRulesContract {
    @Test fun bundledRulesKeepSharingOffUntilTheOwnerEnablesIt() {
        val policy = PhonePolicy()
        val observation = JSONObject().put("idleSeconds", JSONObject.NULL).put("onBattery", false)
            .put("batteryPercent", 100).put("weekday", 1).put("minute", 600)
            .put("resources", JSONObject().put("cpus", 8).put("memoryMib", 8192).put("diskGib", 100))
        val result = NativeRules.evaluate(policy, observation)
        assertFalse(result.allowed)
        assertEquals("Sharing is switched off", result.reason)
        assertTrue(NativeRules.evaluate(policy.copy(enabled = true), observation).allowed)
    }

    @Test fun nativeInputFailureCannotBecomePermissionToRun() {
        assertThrows(IllegalArgumentException::class.java) { NativeRules.request("{}") }
    }
}
