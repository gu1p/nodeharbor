package io.github.gu1p.nodeharbor

import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** The host kills only the owner process between these two instrumentation runs. */
class OwnerDeathContract {
    @Test fun prepareThenWaitForOwnerProcessTermination() = run(recover = false)
    @Test fun recoverTheExistingDiskAfterOwnerDeath() = run(recover = true)

    private fun run(recover: Boolean) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val saved = context.filesDir.resolve("owner-death-fixture.json")
        check(recover == saved.exists()) { "Recover the existing owner-death fixture before creating another" }
        val owner = if (recover) JSONObject(saved.readText()).getString("owner") else UUID.randomUUID().toString()
        require(UUID.fromString(owner).toString() == owner)
        val guest = OwnedGuest(context, context.noBackupFilesDir.resolve("owned-vm-contract-$owner"))
        val progress = context.filesDir.resolve("owner-death-progress.txt")
        val events = mutableListOf<String>()
        fun record(message: String) = synchronized(events) {
            if (events.size < 128) events.add("${SystemClock.elapsedRealtime()}: $message")
            progress.writeText(events.joinToString("\n") + "\n")
        }
        record(if (recover) "Recovering the existing disk after owner death" else "Preparing the owner-death fixture")
        val snapshot = phoneSnapshot(context, true)
        assertTrue("The physical runtime contract needs 2560 MiB available RAM", snapshot.phone.availableMemoryMib >= 2560)
        assertTrue("The runtime contract needs sufficient free storage", snapshot.phone.freeDiskGib >= if (recover) 10 else 26)
        var session: VmSession? = null
        val renewal = Executors.newSingleThreadScheduledExecutor()
        val consoleFile = context.filesDir.resolve("owner-death-private-console.log")
        var consoleBytes = 0
        consoleFile.outputStream().use { console -> WorkerWakeLock(context).use { wake ->
            renewal.scheduleAtFixedRate({ wake.update(true) }, 0, 30, TimeUnit.SECONDS)
            try {
                val receipt = if (recover) {
                    checkNotNull(guest.receipt(owner)).also {
                        assertTrue(it.complete)
                        assertEquals(JSONObject(saved.readText()).getString("receipt"), it.encode())
                    }
                } else {
                    val specification = context.assets.open("lock.json").bufferedReader().use { JSONObject(it.readText()) }.getJSONObject("guest")
                    guest.claim(owner, 15, specification.getString("version"))
                    for (name in listOf("disk.tar.gz", "kernel", "initrd"))
                        context.filesDir.resolve("verified-guest-downloads/$name").copyTo(guest.file(name))
                    guest.prepare(owner, PhonePolicy(), { true }, ::record)
                }
                val started = SystemClock.elapsedRealtime()
                session = VmSession.start(context, owner, PhonePolicy(), guest, { true }, ::record,
                    privateConsole = { bytes -> synchronized(console) {
                        if (consoleBytes + bytes.size <= 4 * 1024 * 1024) {
                            console.write(bytes); console.flush(); consoleBytes += bytes.size
                        }
                    } }, created = { installLifecycleMarker(guest, owner) })
                val active = session!!
                val status = active.status((1_200_000 - (SystemClock.elapsedRealtime() - started)).coerceIn(1, 1_200_000))
                assertEquals(GUEST_CONTROL_REVISION, status.controlRevision)
                assertFalse(status.configured)
                active.renewLease()
                // The status reply can precede its separately drained console bytes.
                val logDeadline = SystemClock.elapsedRealtime() + 5000
                val marker = if (recover) "marker-preserved" else "marker-created"
                while (!consoleFile.readText().contains("NODEHARBOR_TEST $marker") && SystemClock.elapsedRealtime() < logDeadline) Thread.sleep(20)
                assertTrue("The flushed marker must survive owner death", consoleFile.readText().contains("NODEHARBOR_TEST $marker"))
                if (recover) {
                    assertFalse(consoleFile.readText().contains("NODEHARBOR_TEST marker-created"))
                    val stopStarted = SystemClock.elapsedRealtime()
                    assertEquals(ShutdownOutcome.Graceful, active.shutdown(stopStarted + 120_000).get(126, TimeUnit.SECONDS))
                    assertTrue(active.confirmedStopped)
                    assertEquals(receipt, guest.receipt(owner))
                    record("Recovery boot and graceful shutdown passed; shutdown ${SystemClock.elapsedRealtime() - stopStarted} ms")
                } else {
                    saved.outputStream().use { output ->
                        output.write(JSONObject().put("owner", owner).put("receipt", receipt.encode()).toString().toByteArray())
                        output.fd.sync()
                    }
                    record("Ready for host termination of the owner process")
                    val killDeadline = SystemClock.elapsedRealtime() + 60_000
                    while (SystemClock.elapsedRealtime() < killDeadline) { active.renewLease(); Thread.sleep(5000) }
                    fail("The host did not terminate the owner process within its bounded test window")
                }
            } finally {
                renewal.shutdownNow()
                val stopped = session?.halt() ?: true
                if (stopped) {
                    if (guest.receipt(owner) != null) guest.remove(owner, stopped = true, reset = true)
                    saved.delete()
                }
            }
        } }
    }
}
