package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

class GuestSeedContract {
    @Test fun readinessIdentifiesTheActivatedServiceDefinitionAsWellAsItsPythonFile() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val receipt = GuestReceipt("00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002", 15, "test", true)
        val seed = OwnedGuest(context).workerSeed(receipt).toString(Charsets.UTF_8)
        val writes = org.json.JSONObject(seed.substringAfter("#cloud-config\n")).getJSONArray("write_files")
        val service = (0 until writes.length()).map { writes.getJSONObject(it) }
            .single { it.getString("path").endsWith("/nodeharbor-control.service") }.getString("content")
        assertTrue("The revision must come from the unit that actually launched this process",
            service.lineSequence().any { it == "Environment=NODEHARBOR_CONTROL_UNIT_REVISION=$GUEST_CONTROL_REVISION" })
    }

    @Test fun ownerReadinessIncludesNormalPoweroffWithoutAnEarlyTimerDependencyCycle() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val receipt = GuestReceipt("00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002", 15, "test", true)
        val seed = OwnedGuest(context).workerSeed(receipt).toString(Charsets.UTF_8)
        val config = org.json.JSONObject(seed.substringAfter("#cloud-config\n"))
        val writes = config.getJSONArray("write_files")
        fun unit(name: String): String = (0 until writes.length()).map { writes.getJSONObject(it) }
            .single { it.getString("path").endsWith("/$name") }.getString("content")
        fun values(unit: String, key: String) = unit.lineSequence().filter { it.startsWith("$key=") }
            .flatMap { it.substringAfter('=').split(' ').asSequence() }.toSet()
        val control = unit("nodeharbor-control.service")
        val timer = unit("nodeharbor-watchdog.timer")
        for (dependency in listOf("dbus.service", "systemd-logind.service")) {
            assertTrue("Owner commands must wait for $dependency to support ordinary systemctl poweroff", dependency in values(control, "After"))
            assertTrue("The owned channel must bring up its poweroff dependency", dependency in values(control, "Wants") + values(control, "Requires"))
        }
        assertFalse("Poweroff services start after basic.target; the channel cannot precede it", "basic.target" in values(control, "Before"))
        assertTrue("Lease monitoring must wait until its channel accepts renewal", "nodeharbor-control.service" in values(timer, "After"))
        val timerBefore = values(timer, "Before") + if ("no" in values(timer, "DefaultDependencies")) emptySet() else setOf("timers.target", "shutdown.target")
        assertFalse("A timer waiting for normal services cannot delay the early timers.target", "timers.target" in timerBefore)
        assertTrue("The timer must still stop as part of normal guest shutdown", "shutdown.target" in timerBefore)
        assertTrue("Existing disks must replace their previous owned enablement links", config.getJSONArray("runcmd").toString()
            .contains("""["systemctl","reenable","nodeharbor-control.service","nodeharbor-watchdog.timer"]"""))
    }

    @Test fun refreshingAnExistingOwnedDiskChangesOnlyItsSeedAndRetainsEnrollment() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("seed-upgrade-${java.util.UUID.randomUUID()}")
        val owner = "00000000-0000-4000-8000-000000000001"
        val guest = OwnedGuest(context, root)
        try {
            val receipt = guest.claim(owner, 15, "test")
            guest.file("root.img").writeText("previously flushed disk and enrollment marker")
            guest.file("seed.iso").writeText("old seed")
            guest.refreshSeed(receipt, stopped = true)
            assertEquals("previously flushed disk and enrollment marker", guest.file("root.img").readText())
            assertEquals(receipt, guest.receipt(owner))
            val seed = guest.file("seed.iso").readBytes().toString(Charsets.UTF_8)
            assertTrue(seed.contains("control-$GUEST_CONTROL_REVISION"))
            assertTrue(seed.contains("CONTROL_REVISION = '$GUEST_CONTROL_REVISION'"))
            assertThrows(IllegalStateException::class.java) { guest.refreshSeed(receipt, stopped = false) }
            guest.reserveForVm(owner).use {
                val second = OwnedGuest(context, root)
                assertThrows(IllegalStateException::class.java) { second.reserveForVm(owner) }
                assertThrows(IllegalStateException::class.java) { second.refreshSeed(receipt, stopped = true) }
                assertThrows(IllegalStateException::class.java) { second.remove(owner, stopped = true, reset = true) }
            }
            guest.refreshSeed(receipt, stopped = true)
        } finally { root.deleteRecursively() }
    }

    @Test fun aRestartCanRenewItsLeaseBeforeSlowCloudAndNetworkStartup() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val owner = "00000000-0000-4000-8000-000000000001"
        val receipt = GuestReceipt(owner, "00000000-0000-4000-8000-000000000002", 15, "test", true)
        val seed = OwnedGuest(context).workerSeed(receipt).toString(Charsets.UTF_8)
        assertTrue("Watchdog activation must wait for the private control channel", seed.contains("After=nodeharbor-control.service"))
        assertTrue("The private channel must announce readiness before its watchdog starts", seed.contains("Type=notify"))
        assertTrue("Fixed lifecycle events must reach the private VM console", seed.contains("StandardOutput=journal+console"))
        assertTrue("An existing early control service must adopt its refreshed code", seed.contains("""["systemctl","restart","nodeharbor-control.service"]"""))
        assertFalse("A slow network boot cannot prevent renewal of the owner lease", seed.contains("After=network-online.target cloud-config.service"))
    }
}
