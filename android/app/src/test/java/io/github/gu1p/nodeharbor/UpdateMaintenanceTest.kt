package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class UpdateMaintenanceTest {
    @Test fun preparationAndUnknownInventoryNeverCountAsCompletedJobs() {
        assertEquals(UpdateGate.Preparing, updateGate(true, true, false, true, 0, 0))
        assertEquals(UpdateGate.WaitingForController, updateGate(true, false, false, false, 0, 0))
        assertEquals(UpdateGate.WaitingForJobs, updateGate(true, false, false, true, null, 0))
        assertEquals(UpdateGate.WaitingForJobs, updateGate(true, false, false, true, 0, null))
    }

    @Test fun updatesNeverExpireAnActiveJobOrTrustOnlyOneInventory() {
        repeat(10_000) {
            assertEquals(UpdateGate.WaitingForJobs, updateGate(true, false, false, true, 1, 0))
            assertEquals(UpdateGate.WaitingForJobs, updateGate(true, false, false, true, 0, 1))
        }
        assertEquals(UpdateGate.StopWorker, updateGate(true, false, false, true, 0, 0))
    }

    @Test fun installationNeedsConfirmedTerminationAndNoInFlightPreparation() {
        assertEquals(UpdateGate.WaitingForTermination, updateGate(false, false, false, true, 0, 0))
        assertEquals(UpdateGate.Ready, updateGate(false, false, true, false, null, null))
        assertEquals(UpdateGate.Preparing, updateGate(false, true, true, true, 0, 0))
    }
}
