package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Large device contract: real preparation and private control, without fleet access. */
class OwnedVmContract {
    @Test fun verifiedPreparationBootsTheOwnedGuestAndServesOnlyItsOwnerChannel() = lifecycle()
    @Test fun stoppingDuringBootIsBoundedAndTheSameDiskBootsAgain() = lifecycle("boot")
    @Test fun anUnresponsiveGuestIsForcedDownAndTheSameDiskBootsAgain() = lifecycle("unresponsive")
    @Test fun aCrashedGuestIsStoppedAndTheSameDiskBootsAgain() = lifecycle("crash")
    @Test fun anExpiredOwnerLeaseStopsTheGuestAndTheSameDiskBootsAgain() = lifecycle("lease")
    @Test fun anExistingDiskAdoptsTheNewControlRevisionAndPreservesItsOwnerAndData() = lifecycle("upgrade")
    @Test fun forcedStopsRecoverOnTheSameDiskAcrossAllFailureModes() = lifecycle("faults")
    @Test fun recoverCheckpointThenExerciseCrashAndLeaseOnTheSameDisk() = lifecycle("resume-faults")

    private fun lifecycle(fault: String? = null) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val checkpointFile = context.filesDir.resolve("lifecycle-resume-fixture.json")
        val checkpoint = if (fault == "resume-faults") org.json.JSONObject(checkpointFile.readText()) else null
        val owner = checkpoint?.getString("owner") ?: UUID.randomUUID().toString()
        require(UUID.fromString(owner).toString() == owner)
        val guest = OwnedGuest(context, context.noBackupFilesDir.resolve("owned-vm-contract-$owner"))
        val progress = context.filesDir.resolve("owned-vm-contract-progress.txt")
        val events = mutableListOf<String>()
        fun record(message: String) = synchronized(events) {
            if (events.size < 256) events.add("${android.os.SystemClock.elapsedRealtime()}: $message")
            progress.writeText(events.joinToString("\n") + "\n")
        }
        record(if (checkpoint == null) "Starting the verified preparation contract" else "Recovering the checkpointed test disk")
        var session: VmSession? = null
        val renewal = Executors.newSingleThreadScheduledExecutor()
        val console = context.filesDir.resolve("owned-vm-private-console.log").outputStream()
        var consoleBytes = 0
        WorkerWakeLock(context).use { wake ->
            renewal.scheduleAtFixedRate({ wake.update(true) }, 0, 30, TimeUnit.SECONDS)
            try {
                val snapshot = phoneSnapshot(context, true)
                assertTrue("This contract needs sufficient free storage", snapshot.phone.freeDiskGib >= if (checkpoint == null) 26 else 10)
                assertTrue("This contract needs enough available RAM", snapshot.phone.availableMemoryMib >= 2560)
                if (checkpoint == null && InstrumentationRegistry.getArguments().getString("verifiedDownloads") == "true") {
                    val specification = context.assets.open("lock.json").bufferedReader().use { org.json.JSONObject(it.readText()) }.getJSONObject("guest")
                    guest.claim(owner, 15, specification.getString("version"))
                    for (name in listOf("disk.tar.gz", "kernel", "initrd"))
                        context.filesDir.resolve("verified-guest-downloads/$name").copyTo(guest.file(name))
                    // Production preparation still verifies every pinned digest.
                    // Cached inputs do not qualify this device's networking.
                }
                val receipt = if (checkpoint == null) guest.prepare(owner, PhonePolicy(), { true }, ::record)
                    else checkNotNull(guest.receipt(owner)).also { assertEquals(checkpoint.getString("receipt"), it.encode()) }
                assertTrue(receipt.complete)
                val scenarios = when (fault) {
                    null -> List(5) { null }
                    "faults" -> listOf(null, "boot", null, "unresponsive", null, "crash", null, "lease", null)
                    "resume-faults" -> listOf(null, "crash", null, "lease", null)
                    "upgrade" -> listOf("legacy", null)
                    else -> listOf(null, fault, null)
                }
                scenarios.forEachIndexed { it, inject ->
                record("Starting boot ${it + 1} (${inject ?: "normal"})")
                val expectedRevision = if (inject == "legacy") "2" else GUEST_CONTROL_REVISION
                val bootStarted = android.os.SystemClock.elapsedRealtime()
                session = VmSession.start(context, owner, PhonePolicy(), guest, { true }, ::record, privateConsole = { bytes ->
                    synchronized(console) {
                        if (consoleBytes + bytes.size <= 4 * 1024 * 1024) { console.write(bytes); console.flush(); consoleBytes += bytes.size }
                    }
                }, created = { installLifecycleMarker(guest, owner, inject) })
                val active = session!!
                if (inject == "boot") {
                    val started = android.os.SystemClock.elapsedRealtime()
                    assertEquals(ShutdownOutcome.Forced, active.shutdown(started, immediate = true).get(6, TimeUnit.SECONDS))
                    assertTrue("Forced stop must be confirmed within five seconds", android.os.SystemClock.elapsedRealtime() - started <= 5000)
                    record("Forced stop during boot confirmed")
                    return@forEachIndexed
                }
                var status = active.status((1_200_000 - (android.os.SystemClock.elapsedRealtime() - bootStarted)).coerceIn(1, 1_200_000))
                val mode = inject ?: "normal"
                val consoleFile = context.filesDir.resolve("owned-vm-private-console.log")
                val markerDeadline = bootStarted + 1_200_000
                while (status.controlRevision != expectedRevision || !consoleFile.readText().substringAfterLast("NODEHARBOR_TEST mode-").startsWith(mode)) {
                    assertTrue("The refreshed guest must answer before its original boot deadline", android.os.SystemClock.elapsedRealtime() < markerDeadline)
                    active.renewLease()
                    Thread.sleep(1000)
                    status = active.status((markerDeadline - android.os.SystemClock.elapsedRealtime()).coerceIn(1, 15_000))
                }
                assertEquals(expectedRevision, status.controlRevision)
                if (inject == "legacy") assertFalse("The old control revision cannot become ready", workerReady(status.copy(configured = true, running = true)))
                assertTrue("Boot must remain within 20 minutes", android.os.SystemClock.elapsedRealtime() - bootStarted <= 1_200_000)
                synchronized(console) { console.flush() }
                val diagnostics = context.filesDir.resolve("owned-vm-private-console.log").readText()
                assertEquals("A previously flushed marker must survive every restart", if (checkpoint == null) 1 else 0, Regex("NODEHARBOR_TEST marker-created").findAll(diagnostics).count())
                assertTrue("The guest must verify its marker after restart", (it == 0 && checkpoint == null) || diagnostics.contains("NODEHARBOR_TEST marker-preserved"))
                record("Boot ${it + 1} answered through its private owner channel")
                assertFalse(status.configured)
                assertFalse(status.preparing)
                assertFalse(status.running)
                active.renewLease()
                val stopStarted = android.os.SystemClock.elapsedRealtime()
                val result = when (inject) {
                    "crash" -> {
                        Thread.sleep(3000)
                        assertThrows(Exception::class.java) { active.status(5000) }
                        active.shutdown(android.os.SystemClock.elapsedRealtime(), immediate = true).get(6, TimeUnit.SECONDS)
                    }
                    "lease" -> {
                        // Simulate the final successful controller heartbeat,
                        // then stop renewing authority. The owner's production
                        // lease clock must trigger immediate forced teardown.
                        while (controllerLeaseFresh(stopStarted, android.os.SystemClock.elapsedRealtime())) Thread.sleep(100)
                        val expired = android.os.SystemClock.elapsedRealtime()
                        active.shutdown(expired, immediate = true).get(6, TimeUnit.SECONDS).also {
                            assertTrue("Lease expiry must confirm forced teardown within five seconds",
                                android.os.SystemClock.elapsedRealtime() - expired <= 5000)
                        }
                    }
                    else -> active.shutdown(stopStarted + 120_000).get(126, TimeUnit.SECONDS)
                }
                record("Boot ${it + 1}: ${android.os.SystemClock.elapsedRealtime() - bootStarted} ms including shutdown; stop ${android.os.SystemClock.elapsedRealtime() - stopStarted} ms; $result")
                when (inject) {
                    "unresponsive", "crash", "lease" -> assertEquals(ShutdownOutcome.Forced, result)
                    else -> assertEquals("Forced fallback never counts as graceful acceptance", ShutdownOutcome.Graceful, result)
                }
                assertTrue("Guest poweroff must be confirmed by Android process death", active.confirmedStopped)
                assertEquals("Restart and control upgrades must preserve the owned disk receipt", receipt, guest.receipt(owner))
                record("Android confirmed poweroff after boot ${it + 1}")
                }
            } finally {
                renewal.shutdownNow()
                val stopped = session?.halt() ?: true
                if (stopped && guest.receipt(owner) != null) {
                    guest.remove(owner, stopped = true, reset = true)
                    if (checkpoint != null) checkpointFile.delete()
                }
                console.close()
                // This test never requested fleet access, so there is nothing to revoke.
            }
        }
    }
}
