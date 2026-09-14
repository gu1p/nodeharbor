package io.github.gu1p.nodeharbor

enum class ShutdownOutcome { Graceful, Forced, UnexpectedExit, Unconfirmed }
data class ShutdownUpdate(val poweroff: Boolean = false, val force: Boolean = false, val outcome: ShutdownOutcome? = null)

/** One monotonic deadline and one teardown per VM, independent of blocking RPCs. */
class ShutdownCoordinator {
    private var deadline: Long? = null
    private var forcedAt: Long? = null
    private var diedAt: Long? = null
    private var poweroff = false
    private var consoleDone = false
    @Volatile var outcome: ShutdownOutcome? = null
        private set

    @Synchronized fun stop(now: Long, drainDeadline: Long, immediate: Boolean = false): ShutdownUpdate {
        if (outcome != null || diedAt != null) return poll(now)
        val first = deadline == null
        deadline = minOf(deadline ?: Long.MAX_VALUE, drainDeadline, now + 120_000)
        if (immediate) deadline = now
        val update = poll(now)
        return update.copy(poweroff = first && forcedAt == null)
    }
    @Synchronized fun guestPoweredOff() { if (outcome == null && forcedAt == null) poweroff = true }
    @Synchronized fun processDied(now: Long): ShutdownUpdate {
        if (diedAt == null) diedAt = now
        return poll(now)
    }
    @Synchronized fun consoleClosed(now: Long): ShutdownUpdate { consoleDone = true; return poll(now) }
    @Synchronized fun poll(now: Long): ShutdownUpdate {
        val death = diedAt
        if (death != null && (forcedAt != null || consoleDone || now - death >= 1000)) {
            if (outcome == null || outcome == ShutdownOutcome.Unconfirmed)
                outcome = if (forcedAt != null) ShutdownOutcome.Forced
                    else if (deadline != null && death <= deadline!! && poweroff) ShutdownOutcome.Graceful
                    else ShutdownOutcome.UnexpectedExit
        }
        if (outcome != null) return ShutdownUpdate(outcome = outcome)
        if (death == null && forcedAt == null && deadline?.let { now >= it } == true) {
            forcedAt = now
            return ShutdownUpdate(force = true)
        }
        if (forcedAt?.let { now - it >= 5000 } == true) outcome = ShutdownOutcome.Unconfirmed
        return ShutdownUpdate(outcome = outcome)
    }
}

fun workerNeedsWake(ownerEnabled: Boolean, processing: Boolean, stopping: Boolean, alreadyHeld: Boolean): Boolean =
    (ownerEnabled && processing) || (stopping && alreadyHeld)

const val GUEST_CONTROL_REVISION = "6"
