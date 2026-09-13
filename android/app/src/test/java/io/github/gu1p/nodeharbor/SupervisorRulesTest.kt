package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class SupervisorRulesTest {
    @Test fun expiredAuthorityAndCriticalResourcesForceEvenDuringAnExistingDrain() {
        assertFalse(workerMustForceStop(2, 512, true))
        assertTrue(workerMustForceStop(2, 512, false))
        assertTrue(workerMustForceStop(3, 4096, true))
        assertTrue(workerMustForceStop(null, 4096, true))
        assertTrue(workerMustForceStop(0, 511, true))
    }
    @Test fun aWorkerLosingItsServiceStatusMustLoseAdmission() {
        val healthy = GuestStatus(true, false, true, emptyList(), "")
        assertTrue(workerReady(healthy))
        assertFalse(workerReady(healthy.copy(running = false, pods = null)))
        assertFalse(workerReady(healthy.copy(configured = false)))
        assertFalse(workerReady(healthy.copy(preparing = true)))
    }
    @Test fun severeHeatOrMemoryPressureStopsWithoutWaitingForWorkloadDrain() {
        assertEquals(300, workerDrainSeconds(PhonePolicy(), true, 2, 512))
        assertEquals(0, workerDrainSeconds(PhonePolicy(), true, 3, 2048))
        assertEquals(0, workerDrainSeconds(PhonePolicy(), true, null, 2048))
        assertEquals(0, workerDrainSeconds(PhonePolicy(), true, 0, 511))
        assertEquals(0, workerDrainSeconds(PhonePolicy(), false, 0, 2048))
    }
    @Test fun refreshingAnInactiveWorkerCannotReportItReady() {
        assertEquals("Worker force-stopped.", workerDisplayReason(false, false, "Worker force-stopped.", "forced-stop"))
        assertEquals("Shutdown unconfirmed", workerDisplayReason(false, false, "Shutdown unconfirmed", "shutdown-unconfirmed"))
        assertEquals("Sharing is switched off", workerDisplayReason(false, false, "Ready"))
        assertEquals("Open Your phone and resume the worker", workerDisplayReason(false, true, "Ready"))
        assertEquals("Draining current work", workerDisplayReason(true, false, "Draining current work"))
    }
    @Test fun closingTheUiRequiresExplicitBackgroundPermissionFromTheOwner() {
        val saved = StoredState(policy = PhonePolicy(enabled = true))
        assertTrue(phoneWantsExecution(saved, visible = true))
        assertFalse(phoneWantsExecution(saved, visible = false))
        assertTrue(phoneWantsExecution(saved.copy(policy = saved.policy.copy(background = true)), visible = false))
        assertFalse(phoneWantsExecution(saved.copy(userStopped = true), visible = true))
    }
    @Test fun preparationIsExplicitAndDoesNotRequireEnablingWorkloadAdmission() {
        assertFalse(phoneWantsExecution(StoredState(), visible = true))
        assertTrue(phoneWantsExecution(StoredState(prepareRequested = true), visible = true))
    }
    @Test fun controllerLossCannotRenewTheOwnerLeaseIndefinitely() {
        assertTrue(controllerLeaseFresh(1000, 90_999))
        assertFalse(controllerLeaseFresh(1000, 91_000))
        assertFalse(controllerLeaseFresh(null, 1000))
        assertFalse(controllerLeaseFresh(2000, 1000))
    }
    @Test fun drainDeadlineCannotBeExtendedByRepeatedPauseOrPolicyChanges() {
        assertEquals(301_000, drainDeadline(null, 1000, 300))
        assertEquals(301_000, drainDeadline(301_000, 2000, 600))
        assertEquals(2000, drainDeadline(301_000, 2000, 0))
    }
    @Test fun anAllocatedWorkerStillLeavesMemoryForThePhone() {
        val phone = PhoneObservation(33, true, 511, 20, true, 100, false, true, 0, false, true)
        assertFalse(phoneDecision(PhonePolicy(), phone, 0).allowed)
        assertTrue(phoneDecision(PhonePolicy(), phone.copy(availableMemoryMib = 512), 0).allowed)
    }
}
