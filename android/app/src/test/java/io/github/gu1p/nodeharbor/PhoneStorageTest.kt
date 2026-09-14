package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class PhoneStorageTest {
    private val owner = "b560b2c1-090d-4b89-ac94-3f034ffbba56"
    private val pool = "28dcc8e7-b932-4db1-a016-d812e53c9017"
    private val internal = PhoneStorageLocation("internal", "Internal storage", "fs1", 100 * STORAGE_GIB, true)
    private val external = PhoneStorageLocation("primary", "App storage", "fs1", 100 * STORAGE_GIB, true)
    private val disk = PhoneStorageDisk("d0000000001", "internal", 20, "00000000-0000-4000-8000-000000000001")
    private val current = PhoneStoragePool(owner, pool, 7, listOf(disk))

    @Test fun reviewsReserveCompleteCopiesAndNeverDoubleCountOneFilesystem() {
        val target = listOf(disk.copy(location = "primary", gib = 30),
            PhoneStorageDisk("d0000000002", "internal", 40, "00000000-0000-4000-8000-000000000002"))
        val review = reviewPhoneStorage(current, target, listOf(internal, external))
        assertEquals(mapOf("fs1" to 70 * STORAGE_GIB), review.requiredBytes)
        assertFalse(review.requiresBackup)
        assertEquals(8L, review.generation)
        assertThrows(IllegalArgumentException::class.java) {
            reviewPhoneStorage(current, target, listOf(internal.copy(freeBytes = 60 * STORAGE_GIB), external))
        }
        assertEquals(current, PhoneStoragePool.decode(current.encode(), owner))
    }

    @Test fun shrinkAndRemovalRequireAWholePoolBackupAndRebuild() {
        val shrink = reviewPhoneStorage(current, listOf(disk.copy(gib = 15)), listOf(internal))
        assertTrue(shrink.requiresBackup)
        assertEquals(mapOf("fs1" to 15 * STORAGE_GIB), shrink.requiredBytes)
        val second = disk.copy(id = "d0000000002", incarnation = "00000000-0000-4000-8000-000000000002")
        assertTrue(reviewPhoneStorage(current.copy(disks = listOf(disk, second)), listOf(disk), listOf(internal)).requiresBackup)
        assertThrows(IllegalArgumentException::class.java) { reviewPhoneStorage(current, emptyList(), listOf(internal)) }
    }

    @Test fun missingMembersAndInvalidIdentitiesCannotSelectFallbackStorage() {
        assertThrows(IllegalArgumentException::class.java) { reviewPhoneStorage(current, listOf(disk.copy(gib = 30)), listOf(internal.copy(available = false))) }
        assertThrows(IllegalArgumentException::class.java) { PhoneStoragePool.decode(current.encode(), "9e795ac1-a2c9-4a5e-9515-6f0a6138dfb9") }
        assertThrows(IllegalArgumentException::class.java) { current.copy(disks = listOf(disk, disk)).validate(owner) }
        assertThrows(IllegalArgumentException::class.java) { current.copy(disks = listOf(disk.copy(location = "../../other"))).validate(owner) }
        assertThrows(IllegalArgumentException::class.java) { current.copy(generation = -1).validate(owner) }
    }

    @Test fun recoveryRequiresOptInAFullDelayAndCurrentOwnerPermission() {
        assertFalse(storageRecoveryDue(false, false, true, true, 0, 120_000))
        assertFalse(storageRecoveryDue(true, true, true, true, 0, 120_000))
        assertFalse(storageRecoveryDue(true, false, true, true, 0, 119_999))
        assertFalse(storageRecoveryDue(true, false, false, true, 0, 120_000))
        assertFalse(storageRecoveryDue(true, false, true, false, 0, 120_000))
        assertTrue(storageRecoveryDue(true, false, true, true, 0, 120_000))
        assertFalse(storageRecoveryDue(true, false, true, true, 120_001, 120_000))
    }
    @Test fun replacementStorageRetainsOnlyRemainingSelectedAllocationsAndAccountsForOwnedFiles() {
        val lowInternal = internal.copy(freeBytes = 10 * STORAGE_GIB)
        val removable = PhoneStorageLocation("volume-ABCD", "Removable storage", "fs2", 50 * STORAGE_GIB, true, true)
        val disappeared = disk.copy(id = "d0000000002", location = "missing", gib = 15, incarnation = "00000000-0000-4000-8000-000000000002")
        val original = current.copy(disks = listOf(disk, disappeared))
        val replacement = replacementStorage(original, listOf(lowInternal, removable), mapOf("fs1" to 35 * STORAGE_GIB))
        assertEquals(listOf("internal" to 20), replacement?.map { it.location to it.gib })
        assertNull(replacementStorage(original, listOf(lowInternal, removable), emptyMap()))
        assertNull(replacementStorage(original.copy(disks = listOf(disk.copy(gib = 10), disappeared)), listOf(internal, removable), emptyMap()))
        assertNull(replacementStorage(current.copy(disks = listOf(disappeared)), listOf(internal, removable), emptyMap()))
    }

    @Test fun poolDescriptorsPrecedeSeedAndHaveIndependentReadOnlyProbes() {
        val args = guestArguments(PhonePolicy(), listOf(10,11,12,13,14), 15,16,17,
            listOf(18 to 19, 20 to 21)).toList()
        assertTrue(args.indexOf("virtio-blk-pci,drive=storage1") < args.indexOf("virtio-blk-pci,drive=seed"))
        assertTrue(args.contains("fd=18,set=4"))
        assertTrue(args.contains("fd=19,set=4"))
        assertTrue(args.contains("fd=20,set=5"))
        assertThrows(IllegalArgumentException::class.java) {
            guestArguments(PhonePolicy(), listOf(10,11,12,13,14), 15,16,17, listOf(18 to 10))
        }
    }

    @Test fun storageCommandsAreBoundToTheOwnerAndReadinessRequiresTheExactPoolGeneration() {
        val request = org.json.JSONObject().put("deviceId", owner).put("operation", pool).put("action", "backup").put("previousPoolId", pool)
        val encoded = org.json.JSONObject(guestRequest(1, owner, GuestCommand.Storage, request).toString(Charsets.UTF_8))
        assertEquals("backup", encoded.getJSONObject("storage").getString("action"))
        assertFalse(encoded.has("bootstrap"))
        assertThrows(IllegalArgumentException::class.java) { guestRequest(1, owner, GuestCommand.Storage, org.json.JSONObject(request.toString()).put("deviceId", pool)) }
        val status = GuestStatus(true, false, true, emptyList(), "", storageGeneration = 7, storagePoolId = pool)
        assertTrue(workerReady(status, 7, pool))
        assertFalse(workerReady(status, 8, pool))
        assertFalse(workerReady(status, 7, owner))
        assertFalse(workerReady(status.copy(storageGeneration = -1), 7, pool))
    }
}
