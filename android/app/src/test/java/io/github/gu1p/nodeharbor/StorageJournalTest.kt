package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class StorageJournalTest {
    @Test fun aPendingStorageChangeBlocksAutomaticWorkerStartupAfterRecreation() {
        val saved = StoredState(policy = PhonePolicy(enabled = true, background = true), storageOperationPending = true)
        val recovered = StoredState.decode(saved.encode(), saved.installedVersion)
        assertTrue(recovered.storageOperationPending)
        assertFalse(phoneWantsExecution(recovered, true))
        assertFalse(phoneWantsExecution(recovered, false))
    }
    @Test fun aCurrentStorageMaintenanceOwnerRetainsExecutionWhileTheWorkerDrains() {
        val saved = StoredState(policy = PhonePolicy(enabled = true, background = true), storageOperationPending = true)
        assertTrue(phoneWantsExecution(saved, true, maintenance = true))
        assertTrue(phoneWantsExecution(saved, false, maintenance = true))
        assertFalse(phoneWantsExecution(saved.copy(policy = saved.policy.copy(background = false)), false, maintenance = true))
        assertFalse(phoneWantsExecution(saved.copy(userStopped = true), true, maintenance = false))
    }
    @Test fun storageChangesDrainLongRunningServicesWhileMaintenanceBootsDoNotCreateAnotherDrain() {
        assertTrue(workerShouldDrain(true, null, storageMaintenance = true, storageBoot = false))
        assertFalse(workerShouldDrain(true, null, storageMaintenance = true, storageBoot = true))
        assertFalse(workerShouldDrain(true, null, storageMaintenance = false, storageBoot = false))
        assertTrue(workerShouldDrain(false, null, storageMaintenance = true, storageBoot = true))
        assertTrue(workerShouldDrain(true, 300_000, storageMaintenance = false, storageBoot = false))
    }
    private val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
    private val old = PhoneStoragePool(owner, "00000000-0000-4000-8000-000000000001", 3,
        listOf(PhoneStorageDisk.new("internal", 15)))
    private val target = old.copy(generation = 4, disks = listOf(old.disks.single().copy(location = "primary", incarnation = "00000000-0000-4000-8000-000000000002")))

    @Test fun interruptedChangesKeepTheOriginalAndBlockAdmissionUntilGuestVerificationAndDeath() {
        val pending = PhoneStorageChange("00000000-0000-4000-8000-000000000003", old, target, false, "planned")
        val state = PhoneStorageState(owner, pool = old, generation = 3, pending = pending)
        val recovered = PhoneStorageState.decode(state.encode(), owner)
        assertEquals(state, recovered)
        assertEquals(old, recovered.pool)
        assertFalse(recovered.ready)
        assertThrows(IllegalStateException::class.java) { recovered.commit(true) }
        val verified = recovered.copy(pending = pending.copy(phase = "verified"))
        assertThrows(IllegalStateException::class.java) { verified.commit(false) }
        val committed = verified.commit(true)
        assertEquals(target, committed.pool)
        assertEquals(old.disks, committed.retained)
        assertEquals(4L, committed.generation)
        assertTrue(committed.ready)
        assertNull(committed.pending)
    }

    @Test fun savedRecoveryDefaultsOffAndUnknownJournalPhasesPreserveFiles() {
        val state = PhoneStorageState(owner)
        assertFalse(PhoneStorageState.decode(state.encode(), owner).automaticRecovery)
        val pending = PhoneStorageChange("00000000-0000-4000-8000-000000000003", old, target, false, "unknown")
        assertThrows(IllegalArgumentException::class.java) { PhoneStorageState.decode(state.copy(pending = pending).encode(), owner) }
        assertThrows(IllegalArgumentException::class.java) { PhoneStorageState.decode(state.copy(pool = old, generation = 2).encode(), owner) }
    }
    @Test fun committingARebuiltPoolRetainsItsBackupIdentityUntilExplicitCleanup() {
        val id = "00000000-0000-4000-8000-000000000003"
        val pending = PhoneStorageChange(id, old, target.copy(poolId = "00000000-0000-4000-8000-000000000004"), true, "verified")
        val committed = PhoneStorageState(owner, pool = old, generation = 3, pending = pending).commit(true)
        assertEquals(listOf(PhoneStorageBackup(id, old.poolId)), committed.backups)
        assertEquals(committed, PhoneStorageState.decode(committed.encode(), owner))
    }
    @Test fun wholePoolRecoveryKeepsItsOriginalResetRequestAcrossRestarts() {
        val recovery = PhoneStorageRecovery("00000000-0000-4000-8000-000000000003", listOf(PhoneStorageDisk.new("internal", 15)), "resetting")
        val state = PhoneStorageState(owner, pool = old, generation = 3, automaticRecovery = true, recovery = recovery)
        val restored = PhoneStorageState.decode(state.encode(), owner)
        assertEquals(recovery, restored.recovery)
        assertFalse(restored.ready)
        assertThrows(IllegalArgumentException::class.java) {
            PhoneStorageState.decode(state.copy(recovery = recovery.copy(phase = "unknown")).encode(), owner)
        }
    }
}
