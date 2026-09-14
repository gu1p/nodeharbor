package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class AppUpdateFlowTest {
    private class Runtime : AppUpdateRuntime {
        val events = mutableListOf<String>()
        var cancelled = false
        var cancelAt = ""
        var failureAt = ""
        var gates = ArrayDeque(listOf(UpdateGate.WaitingForJobs, UpdateGate.Ready))
        private fun step(name: String) {
            events += name
            if (cancelAt == name) cancelled = true
            check(failureAt != name) { "Injected failure" }
        }
        override fun active() = !cancelled
        override fun downloadAndVerify() = step("verify")
        override fun beginMaintenance() = step("begin")
        override fun readiness(): UpdateGate { step("inspect"); return gates.removeFirst() }
        override fun awaitChange() = step("wait")
        override fun install() = step("install")
        override fun cancelMaintenance() = step("cancel")
    }

    @Test fun verifiedDownloadPrecedesMaintenanceAndInstallationRequiresConfirmedIdle() {
        val runtime = Runtime()
        assertTrue(applyAppUpdate(runtime))
        assertEquals(listOf("verify", "begin", "inspect", "wait", "inspect", "install"), runtime.events)
    }

    @Test fun cancellationDuringDownloadOrAControllerWaitNeverInstalls() {
        for (point in listOf("verify", "begin", "wait")) {
            val runtime = Runtime().apply { cancelAt = point }
            assertFalse(applyAppUpdate(runtime))
            assertFalse(runtime.events.contains("install"))
            if (point != "verify") assertEquals("cancel", runtime.events.last())
        }
    }

    @Test fun failedVerificationNeverPausesWorkAndFailedInstallationReleasesOnlyMaintenance() {
        val untrusted = Runtime().apply { failureAt = "verify" }
        assertThrows(IllegalStateException::class.java) { applyAppUpdate(untrusted) }
        assertEquals(listOf("verify"), untrusted.events)
        val failed = Runtime().apply { failureAt = "install" }
        assertThrows(IllegalStateException::class.java) { applyAppUpdate(failed) }
        assertEquals("cancel", failed.events.last())
    }
}
