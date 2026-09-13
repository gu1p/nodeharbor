package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class ShutdownCoordinatorTest {
    @Test fun repeatedStopsCannotExtendTheOriginalDrainOrRepeatPoweroff() {
        val stop = ShutdownCoordinator()
        assertTrue(stop.stop(1000, 61_000).poweroff)
        assertFalse(stop.stop(2000, 200_000).poweroff)
        assertFalse(stop.poll(60_999).force)
        assertTrue(stop.poll(61_000).force)
        assertFalse(stop.poll(61_001).force)
        assertEquals(ShutdownOutcome.Unconfirmed, stop.poll(66_000).outcome)
    }
    @Test fun evenAnUnlimitedDrainAllowsOnly120SecondsForGuestPoweroff() {
        val stop = ShutdownCoordinator()
        stop.stop(1000, Long.MAX_VALUE)
        assertTrue(stop.poll(121_000).force)
    }
    @Test fun emergencyEscalatesAnInFlightStopWithoutWaitingForItsCommand() {
        val stop = ShutdownCoordinator()
        stop.stop(1000, 121_000)
        assertTrue(stop.stop(1001, 121_000, immediate = true).force)
        assertFalse(stop.stop(1002, 121_000, immediate = true).force)
        assertEquals(ShutdownOutcome.Unconfirmed, stop.poll(6001).outcome)
        assertEquals(ShutdownOutcome.Forced, stop.processDied(6002).outcome)
    }
    @Test fun acknowledgementAndBindingLossCannotConfirmTermination() {
        val stop = ShutdownCoordinator()
        stop.stop(0, 120_000)
        stop.guestPoweredOff()
        assertNull(stop.poll(119_999).outcome)
        stop.poll(120_000)
        assertEquals(ShutdownOutcome.Unconfirmed, stop.poll(125_000).outcome)
    }
    @Test fun gracefulRequiresBothPoweroffAndDeathInEitherCallbackOrder() {
        for (deathFirst in listOf(false, true)) {
            val stop = ShutdownCoordinator()
            stop.stop(0, 120_000)
            if (deathFirst) { stop.processDied(5); stop.guestPoweredOff() }
            else { stop.guestPoweredOff(); stop.processDied(5) }
            assertEquals(ShutdownOutcome.Graceful, stop.consoleClosed(6).outcome)
            assertFalse(stop.stop(10, 120_010, immediate = true).force)
        }
    }
    @Test fun processDeathWithoutPoweroffIsUnexpectedAndNeverGraceful() {
        val stop = ShutdownCoordinator()
        stop.stop(0, 120_000)
        stop.processDied(5)
        assertEquals(ShutdownOutcome.UnexpectedExit, stop.consoleClosed(6).outcome)
        stop.guestPoweredOff()
        assertEquals(ShutdownOutcome.UnexpectedExit, stop.poll(7).outcome)
    }
    @Test fun missingConsoleEofCannotKeepADeadVmPendingForever() {
        val stop = ShutdownCoordinator()
        stop.processDied(5)
        assertEquals(ShutdownOutcome.UnexpectedExit, stop.poll(1005).outcome)
    }
    @Test fun wakeOwnershipIsRetainedOnlyUntilTheBoundedStopFinishes() {
        assertTrue(workerNeedsWake(false, false, true, true))
        assertFalse(workerNeedsWake(true, false, true, false))
        assertFalse(workerNeedsWake(true, false, false, true))
    }
    @Test fun anOldControlRevisionCannotReportReady() {
        val status = GuestStatus(true, false, true, emptyList(), "", controlRevision = "old")
        assertFalse(workerReady(status))
        assertTrue(workerReady(status.copy(controlRevision = GUEST_CONTROL_REVISION)))
    }
}
