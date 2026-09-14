package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import java.nio.file.Files

class PhoneStorageContract {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    @Test fun serviceDestructionInvalidatesStorageCallbacksWhileRetainingTheJournalHold() {
        val directory = context.cacheDir.resolve("storage-destruction-${UUID.randomUUID()}")
        val store = PrivateStore(directory)
        val supervisor = PhoneSupervisor(context, store, { error("No controller is needed for an idle worker") }, { _, _, _, _, _ -> }, {}, {})
        try {
            assertTrue(supervisor.beginStorageMaintenance(19))
            supervisor.close()
            assertThrows(IllegalStateException::class.java) { supervisor.storageMaintenanceGate(19) }
            assertTrue(store.load().storageOperationPending)
        } finally { supervisor.close(); directory.deleteRecursively() }
    }

    @Test fun deletingAllStorageRequiresConfirmedDeathAndRevokedFleetAccess() {
        val directory = context.cacheDir.resolve("storage-delete-${UUID.randomUUID()}")
        val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
        val storage = OwnedPhoneStorage(context, directory)
        try {
            storage.begin(owner, PhoneStorageReview(listOf(PhoneStorageDisk.new("internal", 15)), 1, emptyMap(), false), true)
            val original = storage.load(owner)
            assertThrows(IllegalStateException::class.java) { storage.removeAll(owner, false, true, true) }
            assertThrows(IllegalStateException::class.java) { storage.removeAll(owner, true, false, true) }
            assertEquals(original, storage.load(owner))
            storage.removeAll(owner, true, true, true)
            val removed = storage.load(owner)
            assertEquals(owner, removed.deviceId)
            assertTrue(removed.disabled)
            assertNull(removed.pool)
            assertNull(removed.pending)
            assertTrue(removed.generation > original.generation)
        } finally { directory.deleteRecursively() }
    }
    @Test fun explicitDeletionCancelsAPreviouslyAuthorizedRecovery() {
        val directory = context.cacheDir.resolve("storage-delete-recovery-${UUID.randomUUID()}").apply { mkdir() }
        val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
        try {
            val recovery = PhoneStorageRecovery(UUID.randomUUID().toString(), listOf(PhoneStorageDisk.new("internal", 15)), "rebuilding")
            directory.resolve("pool.json").writeText(PhoneStorageState(owner, generation = 1, disabled = true,
                automaticRecovery = true, recovery = recovery).encode())
            val storage = OwnedPhoneStorage(context, directory)
            storage.removeAll(owner, true, true, true)
            assertNull(storage.load(owner).recovery)
            assertTrue(storage.load(owner).disabled)
        } finally { directory.deleteRecursively() }
    }

    @Test fun storageMaintenanceCannotOverlapAnUpdateOrUndoAnOwnerStop() {
        val directory = context.cacheDir.resolve("storage-owner-${UUID.randomUUID()}")
        val store = PrivateStore(directory)
        val policy = PhonePolicy(enabled = true, background = true)
        store.update { it.copy(policy = policy) }
        val supervisor = PhoneSupervisor(context, store, { error("No controller is needed for an idle worker") }, { _, _, _, _, _ -> }, {}, {})
        try {
            assertTrue(supervisor.beginStorageMaintenance(7))
            assertEquals(UpdateGate.Ready, supervisor.storageMaintenanceGate(7))
            assertFalse(supervisor.beginApplicationUpdate(8))
            supervisor.endStorageMaintenance(6)
            assertTrue(store.load().storageOperationPending)
            store.update { it.copy(userStopped = true, policy = it.policy.copy(enabled = false)) }
            supervisor.ownerStoppedStorage()
            assertThrows(IllegalStateException::class.java) { supervisor.storageMaintenanceGate(7) }
            assertTrue(store.load().userStopped)
            assertFalse(store.load().policy.enabled)
            assertTrue(store.load().storageOperationPending)
        } finally { supervisor.close(); directory.deleteRecursively() }
    }

    @Test fun poolDisksCrossTheSandboxBoundaryOnlyAsOwnedDescriptors() {
        val operation = ISandbox::class.java.methods.singleOrNull { it.name == "bootPool" }
        assertNotNull("The isolated VM needs an explicit pool descriptor capability", operation)
        assertEquals(2, operation!!.parameterTypes.count { it == Array<android.os.ParcelFileDescriptor>::class.java })
        assertFalse(operation.parameterTypes.contains(String::class.java))
    }

    @Test fun pendingStorageSurvivesRecreationAndCannotMutateAnUnconfirmedVm() {
        val directory = context.cacheDir.resolve("storage-journal-${UUID.randomUUID()}")
        val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
        try {
            val storage = OwnedPhoneStorage(context, directory)
            val review = PhoneStorageReview(listOf(PhoneStorageDisk.new("internal", 15)), 1, emptyMap(), false)
            storage.reserveForVm(owner).use {
                assertThrows(IllegalStateException::class.java) { OwnedPhoneStorage(context, directory).begin(owner, review, true) }
            }
            assertThrows(IllegalStateException::class.java) { storage.begin(owner, review, false) }
            val pending = storage.begin(owner, review, true)
            val restored = OwnedPhoneStorage(context, directory).load(owner)
            assertEquals(pending, restored.pending)
            assertNull(restored.pool)
            assertFalse(restored.ready)
            assertThrows(IllegalStateException::class.java) { storage.reserveForVm(owner) }
            assertThrows(IllegalStateException::class.java) { storage.commit(owner, true) }
        } finally { directory.deleteRecursively() }
    }

    @Test fun locationsUseAppSpecificDirectoriesAndIdentifySharedFilesystems() {
        val catalog = PhoneStorageLocations(context)
        val locations = catalog.snapshot()
        assertTrue(locations.any { it.id == "internal" && it.available })
        assertEquals(locations.map { it.id }.distinct().size, locations.size)
        for (location in locations.filter { it.available }) {
            val path = catalog.directory(location.id)
            assertTrue(path.canonicalPath.startsWith(context.noBackupFilesDir.canonicalPath + "/") ||
                context.getExternalFilesDirs(null).filterNotNull().any { path.canonicalPath.startsWith(it.canonicalPath + "/") })
            assertEquals(android.system.Os.stat(path.absolutePath).st_dev.toString(), location.filesystem)
        }
        assertThrows(IllegalArgumentException::class.java) { catalog.directory("../../other") }
    }

    @Test fun copyingAnOwnedDiskVerifiesContentsAndPreservesTheOriginalOnCancellation() {
        val directory = context.cacheDir.resolve("storage-copy-${UUID.randomUUID()}").apply { mkdir() }
        try {
            val source = directory.resolve("source.img").apply { writeBytes(ByteArray(1024 * 1024) { (it % 251).toByte() }) }
            val before = source.readBytes()
            val target = directory.resolve("target.img")
            copyVerifiedStorage(source, target, 2L * 1024 * 1024) { true }
            assertArrayEquals(before, target.readBytes().take(before.size).toByteArray())
            assertEquals(2L * 1024 * 1024, target.length())
            assertArrayEquals(before, source.readBytes())
            val cancelled = directory.resolve("cancelled.img")
            assertThrows(IllegalStateException::class.java) { copyVerifiedStorage(source, cancelled, source.length()) { false } }
            assertArrayEquals(before, source.readBytes())
            assertFalse(cancelled.exists())
            val link = directory.resolve("linked.img")
            Files.createSymbolicLink(link.toPath(), source.toPath())
            assertThrows(IllegalStateException::class.java) { copyVerifiedStorage(link, directory.resolve("unsafe.img"), source.length()) { true } }
            assertArrayEquals(before, source.readBytes())
        } finally { directory.deleteRecursively() }
    }

    @Test fun bootSeedCarriesStorageHelpersForExistingOwnedDisks() {
        val guest = OwnedGuest(context)
        val seed = guest.workerSeed(GuestReceipt("9511182e-9c48-4d20-a15b-1da8bb441386",
            "00000000-0000-4000-8000-000000000001", 15, "ubuntu-24.04-20260911", true)).toString(Charsets.UTF_8)
        val writes = org.json.JSONObject(seed.substringAfter("#cloud-config\n")).getJSONArray("write_files")
        val paths = (0 until writes.length()).map { writes.getJSONObject(it).getString("path") }
        assertTrue(paths.contains("/usr/local/lib/nodeharbor/storage_pool.py"))
        assertTrue(paths.contains("/usr/local/lib/nodeharbor/storage_backup.py"))
    }
}
