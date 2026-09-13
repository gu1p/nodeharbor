package io.github.gu1p.nodeharbor

import android.content.Context
import android.os.SystemClock
import org.json.JSONObject
import java.io.Closeable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** Fast owner supervision never waits for controller HTTP, disk setup, or guest RPC. */
class PhoneSupervisor(
    private val context: Context,
    private val store: PrivateStore,
    private val client: () -> ControllerClient,
    private val publish: (String, String, Boolean, Boolean) -> Unit,
    private val record: (String) -> Unit,
    private val reportError: (String) -> Unit,
) : Closeable {
    @Volatile var visible = false
    @Volatile private var attached = false
    @Volatile private var allowed = false
    @Volatile private var admission = false
    @Volatile private var remotePaused = true
    @Volatile private var lastHeartbeat: Long? = null
    @Volatile private var vm: VmSession? = null
    @Volatile private var allocatingVm = false
    @Volatile private var ready = false
    @Volatile private var status: GuestStatus? = null
    @Volatile private var statusAt = 0L
    @Volatile private var stage = "paused"
    @Volatile private var reason = "Sharing is switched off"
    @Volatile private var failure = ""
    @Volatile private var deadline: Long? = null
    private var initialDrainSeconds = 0
    @Volatile private var systemPods = emptySet<String>()
    @Volatile private var drained = false
    @Volatile private var resumed = false
    @Volatile private var eligibleCi = false
    @Volatile private var eligibleServices = false
    private val busy = AtomicBoolean(false)
    private val stopping = AtomicBoolean(false)
    private var guard = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-owner-guard").apply { isDaemon = true } }
    private var network = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-fleet").apply { isDaemon = true } }
    private var work = Executors.newSingleThreadExecutor { Thread(it, "nodeharbor-worker-setup").apply { isDaemon = true } }
    private var lease = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-owner-lease").apply { isDaemon = true } }
    private val wake = WorkerWakeLock(context)
    private val guest = OwnedGuest(context)
    @Volatile private var finish: () -> Unit = {}
    private var lastPoll = 0L
    @Volatile private var epoch = 0L
    val confirmedIdle: Boolean get() = vm == null && !busy.get() && !stopping.get()
    val supervised: Boolean get() = attached
    val lifecycleStopping: Boolean get() = stopping.get()
    @Volatile private var lastStop: ShutdownOutcome? = null

    init { schedule() }
    private fun schedule() {
        guard.scheduleWithFixedDelay({ if (attached) checked { tick() } }, 0, 1, TimeUnit.SECONDS)
        network.scheduleWithFixedDelay({ if (attached) fleetTick() }, 0, 15, TimeUnit.SECONDS)
        lease.scheduleWithFixedDelay({
            if (attached && allowed && !stopping.get() && statusAt > 0 && (controllerLeaseFresh(lastHeartbeat, SystemClock.elapsedRealtime()) || deadline != null))
                runCatching { vm?.renewLease() }
        }, 1, 25, TimeUnit.SECONDS)
    }

    @Synchronized fun attach(onFinished: () -> Unit) {
        finish = onFinished
        if (guard.isShutdown) {
            guard = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-owner-guard").apply { isDaemon = true } }
            network = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-fleet").apply { isDaemon = true } }
            work = Executors.newSingleThreadExecutor { Thread(it, "nodeharbor-worker-setup").apply { isDaemon = true } }
            lease = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-owner-lease").apply { isDaemon = true } }
            schedule()
        }
        attached = true
    }
    fun retry() { failure = ""; lastStop = null }
    fun ownedDiskGib(owner: String): Long = guest.allocatedBytes(owner) / (1024L * 1024 * 1024)
    private fun checked(action: () -> Unit) {
        try { action() } catch (error: Exception) { fail(error) }
    }
    private fun fail(error: Exception) {
        val message = when (error) {
            is ControllerFailure -> error.message!!
            is java.io.IOException -> "Worker setup could not reach its verified downloads or fleet. Check the connection and retry."
            is org.json.JSONException -> "The worker or fleet returned an invalid response"
            else -> error.message ?: "The owned worker could not complete this operation"
        }.take(2048)
        if (failure != message) reportError(message)
        failure = message
        allowed = false; admission = false
        if (error is ControllerFailure && error.status in listOf(401, 403))
            store.update { it.copy(policy = it.policy.copy(enabled = false), userStopped = true, prepareRequested = false) }
        halt()
    }

    @Synchronized private fun tick() {
        val saved = store.load()
        val now = SystemClock.elapsedRealtime()
        val active = vm
        val snapshot = phoneSnapshot(context, runtimeReady = true, ownedDiskGib = ownedDiskGib(saved.deviceId))
        val phone = phoneDecision(saved.policy, snapshot.phone, if (active == null && !allocatingVm) saved.policy.memoryMib else 0)
        val shared = NativeRules.evaluate(saved.policy.copy(enabled = true), snapshot.shared)
        val wants = phoneWantsExecution(saved, visible) && saved.resetRequest.isEmpty()
        val fresh = controllerLeaseFresh(lastHeartbeat, now)
        val resourcesChanged = active != null && (active.policy.cpus != saved.policy.cpus ||
            active.policy.memoryMib != saved.policy.memoryMib || active.policy.diskGib != saved.policy.diskGib)
        val desired = wants && phone.allowed && shared.allowed && snapshot.phone.freeDiskGib >= 10 &&
            fresh && !remotePaused && failure.isEmpty() && !resourcesChanged
        reason = when {
            failure.isNotEmpty() -> failure
            saved.resetRequest.isNotEmpty() -> "Stopping the owned worker before removing its access"
            !wants -> if (!visible && !saved.policy.background && !saved.userStopped) "Paused after closing the app" else "Sharing is switched off"
            !phone.allowed -> phone.reason
            !shared.allowed -> shared.reason
            snapshot.phone.freeDiskGib < 10 -> "The phone needs at least 10 GiB of free storage"
            !fresh -> "Waiting for the fleet controller"
            remotePaused -> "Paused by your fleet administrator"
            resourcesChanged -> "Draining before applying the new resource limits"
            !ready -> if (busy.get()) reason else "Preparing the owned worker"
            !saved.policy.enabled -> "Worker prepared. Enable sharing when you are ready."
            eligibleServices && saved.policy.allowServices -> "Sharing with your fleet"
            eligibleCi && saved.policy.allowCi -> "Ready for ARM64 CI jobs"
            else -> "Connected. The fleet is observing worker reliability."
        }
        admission = desired && ready && saved.policy.enabled && deadline == null
        if (active != null && !active.running && !stopping.get()) {
            if (desired && deadline == null) failure = "The isolated worker stopped unexpectedly. Check the activity and retry."
            halt()
        }
        val emergency = active != null && workerMustForceStop(snapshot.phone.thermal, snapshot.phone.availableMemoryMib, fresh)
        if (emergency) halt()
        if (active != null && !stopping.get() && (!desired || deadline != null)) {
            if (deadline == null) {
                val limit = workerDrainSeconds(saved.policy, ready, snapshot.phone.thermal, snapshot.phone.availableMemoryMib)
                deadline = drainDeadline(null, now, limit)
                initialDrainSeconds = limit
                val wall = System.currentTimeMillis()
                store.update { it.copy(drainingSince = wall / 1000, drainDeadlineMillis = drainDeadline(it.drainDeadlineMillis, wall, limit)) }
                drained = false; resumed = false
                record("Stopped accepting work; draining for at most $limit seconds")
            }
            val remaining = minOf(deadline!! - now, (store.load().drainDeadlineMillis ?: 0) - System.currentTimeMillis()).coerceAtLeast(0)
            val workloads = if (drained && now - statusAt < 30_000) status?.workloadCount(systemPods) ?: Int.MAX_VALUE else Int.MAX_VALUE
            val transition = NativeRules.transition(false, true, 0, (initialDrainSeconds.toLong() - (remaining + 999) / 1000).coerceAtLeast(0),
                initialDrainSeconds, workloads)
            if (remaining == 0L || transition == "stop") { allowed = false; halt(immediate = remaining == 0L, stopDeadline = now + remaining) }
            else { allowed = true; stage = "draining"; reason = "Draining current work (${(remaining + 999) / 1000}s remaining)" }
        } else if (active == null && !stopping.get()) {
            allowed = desired
            stage = if (failure.isNotEmpty()) "error" else if (busy.get()) "preparing" else "paused"
            if (saved.resetRequest.isNotEmpty() && failure.isEmpty()) begin { replace(saved) }
            else if (desired) begin { prepareAndStart(saved) }
        } else if (desired && deadline == null && !stopping.get()) {
            allowed = true
            stage = if (!ready) "preparing" else if (admission && (eligibleCi || eligibleServices)) "sharing" else "connecting"
        }
        if (active != null && now - lastPoll >= 10_000 && !stopping.get()) {
            lastPoll = now
            begin { updateGuest(active) }
        }
        if (active == null && !desired && lastStop == ShutdownOutcome.Forced && failure.isEmpty()) {
            stage = "forced-stop"; reason = "Worker force-stopped."
        }
        if (stopping.get()) {
            val unconfirmed = active?.shutdownOutcome == ShutdownOutcome.Unconfirmed
            stage = if (unconfirmed) "shutdown-unconfirmed" else "stopping"
            reason = if (unconfirmed) "Shutdown unconfirmed. Worker files have been preserved. Restart and replacement are blocked."
                else "Stopping. Waiting for guest poweroff and Android process death."
        }
        wake.update(workerNeedsWake(saved.policy.preventSleep, allowed && (active != null || busy.get()),
            stopping.get() && active?.shutdownOutcome == null, wake.held))
        publish(stage, reason, eligibleCi, eligibleServices)
        if (!wants && saved.resetRequest.isEmpty() && vm == null && !busy.get() && !stopping.get()) finish()
    }

    private fun begin(action: () -> Unit) {
        if (!busy.compareAndSet(false, true)) return
        val operationEpoch = epoch
        work.execute { try { action() } catch (error: Exception) {
            synchronized(this) { if (epoch == operationEpoch && (allowed || store.load().resetRequest.isNotEmpty())) fail(error) }
        }
            finally { busy.set(false) } }
    }
    private fun prepareAndStart(saved: StoredState) {
        check(saved.deviceId.isNotEmpty()) { "Connect this phone to your fleet first" }
        stage = "preparing"
        val operationEpoch = epoch
        val receipt = guest.prepare(saved.deviceId, saved.policy, { epoch == operationEpoch && attached && allowed && phoneWantsExecution(store.load(), visible) }) {
            if (epoch == operationEpoch) { reason = it; record(it) }
        }
        check(allowed && attached && phoneWantsExecution(store.load(), visible)) { "Worker preparation was stopped" }
        val preflight = phoneDecision(saved.policy, phoneSnapshot(context, true).phone, saved.policy.memoryMib)
        check(preflight.allowed) { preflight.reason }
        val bootDeadline = SystemClock.elapsedRealtime() + 1_200_000
        allocatingVm = true
        val session = try {
            VmSession.start(context, saved.deviceId, saved.policy, guest, { attached && allowed && epoch == operationEpoch },
                { if (epoch == operationEpoch) record(it) }, created = { created ->
                    synchronized(this) {
                        vm = created
                        created.observeShutdown { outcome -> stopped(created, outcome) }
                        if (!attached || !allowed || epoch != operationEpoch) halt()
                    }
                })
        } finally { allocatingVm = false }
        if (!allowed || !attached) { halt(); return }
        store.update { it.copy(workerGeneration = receipt.generation) }
        reason = "Starting Ubuntu. The first boot may take several minutes."
        // This can wait for boot, but the independent owner guard can revoke it.
        var first = session.status((bootDeadline - SystemClock.elapsedRealtime()).coerceIn(1, 1_200_000))
        while (first.controlRevision != GUEST_CONTROL_REVISION) {
            check(current(session, operationEpoch)) { "The owner stopped VM startup" }
            check(SystemClock.elapsedRealtime() < bootDeadline) { "The guest control update did not become ready before its boot deadline" }
            session.renewLease()
            Thread.sleep(1000)
            first = session.status(minOf(15_000, (bootDeadline - SystemClock.elapsedRealtime()).coerceAtLeast(1)))
        }
        if (!current(session, operationEpoch)) return
        session.renewLease()
        synchronized(this) {
            if (!current(session, operationEpoch)) return
            status = first; statusAt = SystemClock.elapsedRealtime()
        }
        if (!first.configured) {
            check(allowed && controllerLeaseFresh(lastHeartbeat, SystemClock.elapsedRealtime()) && !remotePaused)
            val bootstrap = client().request("/device/bootstrap", JSONObject()) as JSONObject
            check(allowed && attached && phoneWantsExecution(store.load(), visible))
            if (!current(session, operationEpoch)) return
            session.configure(bootstrap)
            if (!current(session, operationEpoch)) return
            reason = "Installing the private network and Kubernetes worker inside Ubuntu"
            record(reason)
        } else updateGuest(session)
    }
    private fun current(session: VmSession, expectedEpoch: Long): Boolean = vm === session && epoch == expectedEpoch && !stopping.get() && attached
    private fun updateGuest(session: VmSession) {
        val operationEpoch = epoch
        if (!session.running || !current(session, operationEpoch)) return
        val observed = session.status()
        synchronized(this) {
        if (!current(session, operationEpoch)) return
        status = observed; statusAt = SystemClock.elapsedRealtime()
        check(observed.error.isEmpty()) { observed.error }
        val wasReady = ready
        ready = workerReady(observed)
        if (!ready) admission = false
        if (ready && !wasReady) {
            store.update { it.copy(workerInstalled = true, prepareRequested = false) }
            record("The owned ARM64 worker is configured. Fleet qualification remains required.")
        }
    }

    }

    private fun fleetTick() {
        val operationEpoch = epoch
        if (stopping.get()) return
        try {
            val saved = store.load()
            if (saved.deviceId.isEmpty()) return
            val controller = client()
            val allocation = vm?.policy ?: saved.policy
            val result = controller.request("/heartbeat", JSONObject().put("state", stage).put("reason", reason.take(2048))
                .put("resources", allocation.sharedJson().getJSONObject("resources"))
                .put("allowCi", saved.policy.allowCi).put("allowServices", saved.policy.allowServices)
                .put("permitted", admission)) as JSONObject
            synchronized(this) {
                if (operationEpoch != epoch || !attached || stopping.get()) return
            remotePaused = result.getBoolean("remotePaused")
            eligibleCi = result.getBoolean("eligibleCi"); eligibleServices = result.getBoolean("eligibleServices")
            lastHeartbeat = SystemClock.elapsedRealtime()
            }
            if (deadline != null && !drained) {
                val resultDrain = controller.request("/device/drain", JSONObject()) as JSONObject
                check(resultDrain.getBoolean("draining"))
                val pods = resultDrain.getJSONArray("systemPodUids")
                require(pods.length() <= 4096)
                val observedPods = (0 until pods.length()).map { pods.getString(it).also { uid -> require(uid.length in 1..128) } }.toSet()
                synchronized(this) {
                    if (operationEpoch != epoch || !attached || stopping.get()) return
                    systemPods = observedPods; drained = true
                }
            } else if (admission && !remotePaused && !resumed && deadline == null) {
                val resultResume = controller.request("/device/resume", JSONObject()) as JSONObject
                check(resultResume.getBoolean("observing"))
                synchronized(this) { if (operationEpoch == epoch && attached && !stopping.get()) resumed = true }
            }
        } catch (error: ControllerFailure) {
            synchronized(this) { if (operationEpoch == epoch && attached && !stopping.get() && error.status in listOf(401, 403)) fail(error) }
            // Other failures expire the monotonic controller lease normally.
        } catch (_: Exception) { /* Owner supervision continues while HTTP is unavailable. */ }
    }
    @Synchronized private fun halt(immediate: Boolean = true, stopDeadline: Long = SystemClock.elapsedRealtime()) {
        val session = vm ?: return
        if (stopping.compareAndSet(false, true)) {
            epoch++
            allowed = false; admission = false; ready = false
            stage = "stopping"; reason = "Stopping. Waiting for guest poweroff and Android process death."
            publish(stage, reason, false, false)
        }
        session.shutdown(stopDeadline, immediate)
    }
    @Synchronized private fun stopped(session: VmSession, outcome: ShutdownOutcome) {
        if (vm !== session) return
        lastStop = outcome
        allowed = false; admission = false; ready = false
        wake.update(false)
        when (outcome) {
            ShutdownOutcome.Unconfirmed -> {
                stopping.set(true)
                stage = "shutdown-unconfirmed"
                failure = "Shutdown unconfirmed. Worker files have been preserved. Restart and replacement are blocked."
                reason = failure; reportError(failure)
            }
            else -> {
                epoch++
                if (failure.startsWith("Shutdown unconfirmed.")) failure = ""
                vm = null; status = null; statusAt = 0; resumed = false; drained = false
                deadline = null; systemPods = emptySet(); stopping.set(false)
                store.update { it.copy(drainingSince = null, drainDeadlineMillis = null) }
                when (outcome) {
                    ShutdownOutcome.Graceful -> { stage = "paused"; reason = "Worker stopped gracefully." }
                    ShutdownOutcome.Forced -> { stage = "forced-stop"; reason = "Worker force-stopped." }
                    else -> { stage = "error"; failure = "The isolated worker exited without confirmed guest poweroff. Check the activity and retry."; reason = failure }
                }
                record(reason)
            }
        }
        publish(stage, reason, false, false)
    }
    private fun replace(saved: StoredState) {
        check(vm == null && !stopping.get()) { "Wait for Android to confirm worker shutdown" }
        val reset = client().request("/device/reset", JSONObject().put("requestId", saved.resetRequest)) as JSONObject
        check(reset.getBoolean("complete")) { "The fleet is still removing the previous worker's access" }
        if (guest.receipt(saved.deviceId) != null) guest.remove(saved.deviceId, stopped = true, reset = true)
        store.update { it.copy(workerInstalled = false, workerGeneration = "", resetRequest = "", prepareRequested = false,
            policy = it.policy.copy(enabled = false), userStopped = true) }
        record("The previous worker was removed. Your enrollment and sharing rules were preserved.")
    }
    @Synchronized override fun close() {
        attached = false; allowed = false; admission = false
        epoch++
        halt()
        guard.shutdownNow(); network.shutdownNow(); lease.shutdownNow(); work.shutdownNow()
        if (vm == null) wake.update(false)
    }
}
