package io.github.gu1p.nodeharbor

import android.os.Bundle
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Real Ubuntu/LVM acceptance in an unenrolled, privately owned instrumentation guest. */
class PoolVmContract {
    @Test fun twoDisksPreserveDataAcrossVerifiedGrowthAndGracefulRestart() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val owner = UUID.randomUUID().toString()
        val guest = OwnedGuest(context, context.noBackupFilesDir.resolve("owned-vm-contract-pool-$owner"))
        val metadata = context.cacheDir.resolve("pool-vm-contract-$owner")
        val storage = OwnedPhoneStorage(context, metadata)
        val progress = context.filesDir.resolve("pool-vm-progress.txt")
        val events = mutableListOf<String>()
        fun record(message: String) = synchronized(events) {
            events.add("${SystemClock.elapsedRealtime()}: $message")
            progress.writeText(events.takeLast(256).joinToString("\n") + "\n")
        }
        var session: VmSession? = null
        val renewal = Executors.newSingleThreadScheduledExecutor()
        val console = context.filesDir.resolve("pool-vm-private-console.log").outputStream()
        var consoleBytes = 0
        val report = Bundle()
        WorkerWakeLock(context).use { wake ->
            renewal.scheduleAtFixedRate({ wake.update(true) }, 0, 30, TimeUnit.SECONDS)
            try {
                val snapshot = phoneSnapshot(context, true)
                assertTrue("Real storage acceptance requires the existing 2.5 GiB memory allowance", snapshot.phone.availableMemoryMib >= 2560)
                assertTrue("Real storage acceptance needs space for its owned root, pool, and verified copy", snapshot.phone.freeDiskGib >= 50)
                record("Preparing the verified unenrolled Ubuntu guest")
                if (InstrumentationRegistry.getArguments().getString("verifiedDownloads") == "true") {
                    val spec = context.assets.open("lock.json").bufferedReader().use { JSONObject(it.readText()) }.getJSONObject("guest")
                    guest.claim(owner, 15, spec.getString("version"))
                    for (name in listOf("disk.tar.gz", "kernel", "initrd"))
                        context.filesDir.resolve("verified-guest-downloads/$name").copyTo(guest.file(name))
                }
                val root = guest.prepare(owner, PhonePolicy(), { true }, ::record)
                var previous: PhoneStoragePool? = null
                var selection = listOf(PhoneStorageDisk.new("internal", 5), PhoneStorageDisk.new("internal", 10))
                repeat(2) { cycle ->
                    assertTrue("Each VM boot retains the existing memory qualification", phoneSnapshot(context, true).phone.availableMemoryMib >= 2560)
                    val change = storage.begin(owner, storage.review(owner, selection), true)
                    storage.prepare(owner, true, { true }, ::record)
                    val boot = SystemClock.elapsedRealtime()
                    record("Boot ${cycle + 1}: starting the isolated VM with two owned pool disks")
                    val active = VmSession.start(context, owner, PhonePolicy(diskGib = change.target.gib), guest, { true }, ::record,
                        privateConsole = { bytes -> synchronized(console) {
                            if (consoleBytes + bytes.size <= 4 * 1024 * 1024) { console.write(bytes); console.flush(); consoleBytes += bytes.size }
                        } }, storage = storage, storageMaintenance = true,
                        created = { installPoolFixture(guest, owner) }).also { session = it }
                    var observed = active.status(1_200_000)
                    while (observed.controlRevision != GUEST_CONTROL_REVISION) {
                        assertTrue("Boot exceeded its original twenty-minute deadline", SystemClock.elapsedRealtime() < boot + 1_200_000)
                        active.renewLease(); Thread.sleep(1000); observed = active.status()
                    }
                    report.putString("poolBoot${cycle + 1}Millis", (SystemClock.elapsedRealtime() - boot).toString())
                    active.renewLease()
                    val lease = renewal.scheduleAtFixedRate({ runCatching { active.renewLease() } }, 10, 20, TimeUnit.SECONDS)
                    val operation = SystemClock.elapsedRealtime()
                    active.storage(JSONObject().put("deviceId", owner).put("operation", change.id).put("action", "apply")
                        .put("previousPoolId", previous?.poolId ?: JSONObject.NULL).put("pool", change.target.guestRequest(previous)).put("restore", false))
                    record("Boot ${cycle + 1}: applying the journaled storage pool inside Ubuntu")
                    var result: JSONObject? = null
                    while (result == null) {
                        assertTrue("Guest storage work exceeded twenty minutes", SystemClock.elapsedRealtime() < operation + 1_200_000)
                        observed = active.status()
                        assertEquals("", observed.storageError)
                        result = observed.storageResult?.takeIf { it.optString("operation") == change.id }
                        if (result == null) Thread.sleep(1000)
                    }
                    assertEquals(change.target.generation, result.getLong("generation"))
                    assertEquals(change.target.poolId, result.getString("poolId"))
                    assertTrue(result.getLong("capacityBytes") >= change.target.gib * STORAGE_GIB * 9 / 10)
                    assertEquals(cycle == 0, result.getBoolean("testMarkerCreated"))
                    report.putString("poolApply${cycle + 1}Millis", (SystemClock.elapsedRealtime() - operation).toString())
                    lease.cancel(false)
                    val stop = SystemClock.elapsedRealtime()
                    assertEquals("Forced fallback does not qualify graceful shutdown", ShutdownOutcome.Graceful,
                        active.shutdown(stop + 120_000).get(126, TimeUnit.SECONDS))
                    assertTrue(active.confirmedStopped)
                    report.putString("poolStop${cycle + 1}Millis", (SystemClock.elapsedRealtime() - stop).toString())
                    storage.markVerified(owner, change.id, result.getLong("generation"), result.getString("poolId"), true)
                    storage.commit(owner, true)
                    assertEquals(root, guest.receipt(owner))
                    record("Boot ${cycle + 1}: pool data verified and graceful process death confirmed")
                    previous = change.target
                    selection = change.target.disks.mapIndexed { index, disk -> if (index == 0) disk.copy(gib = 6) else disk }
                }
                assertTrue(storage.load(owner).retained.isNotEmpty())
                report.putString("poolMarkerPreserved", "true")
                report.putString("poolRootReceiptPreserved", "true")
                report.putString("poolDiskCount", "2")
                instrumentation.sendStatus(0, report)
            } finally {
                renewal.shutdownNow()
                val stopped = session?.halt() ?: true
                if (stopped) {
                    storage.removeAll(owner, true, true, true)
                    if (guest.receipt(owner) != null) guest.remove(owner, true, true)
                    metadata.deleteRecursively()
                }
                console.close()
            }
        }
    }
}
