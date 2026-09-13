package io.github.gu1p.nodeharbor

fun phoneWantsExecution(saved: StoredState, visible: Boolean): Boolean =
    !saved.userStopped && (saved.policy.enabled || saved.prepareRequested) && (visible || saved.policy.background)

/** Monotonic time only: a clock correction must never extend fleet authority. */
fun controllerLeaseFresh(lastSuccess: Long?, now: Long): Boolean =
    lastSuccess != null && now >= lastSuccess && now - lastSuccess < 90_000

fun drainDeadline(existing: Long?, now: Long, seconds: Int): Long =
    minOf(existing ?: Long.MAX_VALUE, now + seconds.coerceAtLeast(0).toLong() * 1000)

fun workerDrainSeconds(policy: PhonePolicy, ready: Boolean, thermal: Int?, availableMemoryMib: Long): Int =
    if (!ready || thermal == null || thermal >= 3 || availableMemoryMib < 512) 0 else policy.drainSeconds

fun workerMustForceStop(thermal: Int?, availableMemoryMib: Long, authorityFresh: Boolean): Boolean =
    !authorityFresh || thermal == null || thermal >= 3 || availableMemoryMib < 512

fun workerDisplayReason(supervised: Boolean, enabled: Boolean, current: String, lifecycleState: String = ""): String = when {
    lifecycleState in listOf("stopping", "shutdown-unconfirmed", "forced-stop") -> current
    supervised -> current
    !enabled -> "Sharing is switched off"
    else -> "Open Your phone and resume the worker"
}

fun workerReady(status: GuestStatus): Boolean =
    status.configured && status.running && !status.preparing && status.error.isEmpty() && status.controlRevision == GUEST_CONTROL_REVISION
