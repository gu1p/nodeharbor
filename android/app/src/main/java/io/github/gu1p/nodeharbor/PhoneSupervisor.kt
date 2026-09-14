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
    private val publish: (String, String, Boolean, Boolean, List<GuestPod>?) -> Unit,
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
    private val phoneStorage = OwnedPhoneStorage(context)
    @Volatile private var finish: () -> Unit = {}
    private var lastPoll = 0L
    @Volatile private var epoch = 0L
    val confirmedIdle: Boolean get() = vm == null && !busy.get() && !stopping.get()
    val supervised: Boolean get() = attached
    val lifecycleStopping: Boolean get() = stopping.get()
    @Volatile private var lastStop: ShutdownOutcome? = null
    private var updateOwner: Long? = null
    private var maintenanceAt = 0L
    private var maintenanceWorkloads: Int? = null
    @Volatile private var storageOwner: Long? = null
    @Volatile private var storageBootRequested = false

    @Synchronized fun beginStorageMaintenance(owner: Long): Boolean {
        val saved = store.load()
        if (saved.applicationUpdatePending || updateOwner != null || busy.get() || allocatingVm || saved.prepareRequested || saved.resetRequest.isNotEmpty()) return false
        check(storageOwner == null || storageOwner == owner) { "Another storage operation is active" }
        storageOwner = owner; storageBootRequested = false
        maintenanceAt = 0; maintenanceWorkloads = null; resumed = false; admission = false
        store.update { it.copy(storageOperationPending = true) }
        return true
    }
    @Synchronized fun storageMaintenanceGate(owner: Long): UpdateGate {
        check(storageOwner == owner) { "The owner stopped storage maintenance" }
        check(failure.isEmpty()) { failure }
        if (stopping.get()) return UpdateGate.WaitingForTermination
        // The supervisor owns the original drain deadline and termination. A
        // polling storage operation must never extend it or start another stop.
        return when {
            confirmedIdle -> UpdateGate.Ready
            busy.get() || allocatingVm -> UpdateGate.Preparing
            else -> UpdateGate.WaitingForJobs
        }
    }
    @Synchronized fun requestStorageBoot(owner: Long) {
        check(storageOwner == owner && confirmedIdle && attached) { "Confirm worker shutdown before starting storage maintenance" }
        phoneStorage.vmFiles(store.load().deviceId, maintenance = true)
        failure = ""; lastStop = null; storageBootRequested = true
    }
    @Synchronized fun storageSession(owner: Long): VmSession? {
        check(storageOwner == owner) { "The owner stopped storage maintenance" }
        check(failure.isEmpty()) { failure }
        return vm?.takeIf { !busy.get() && !stopping.get() && status?.controlRevision == GUEST_CONTROL_REVISION }
    }
    @Synchronized fun stopStorageVm(owner: Long) {
        check(storageOwner == owner) { "The owner stopped storage maintenance" }
        storageBootRequested = false
        halt(false, SystemClock.elapsedRealtime() + 120_000)
    }
    @Synchronized fun endStorageMaintenance(owner: Long) {
        if (storageOwner != owner) return
        check(confirmedIdle) { "Wait for Android to confirm worker shutdown" }
        storageOwner = null; storageBootRequested = false; maintenanceAt = 0; maintenanceWorkloads = null; resumed = false
        store.update { it.copy(storageOperationPending = it.deviceId.isNotEmpty() && phoneStorage.load(it.deviceId).let { state -> state.pending != null || state.recovery != null }) }
    }
    @Synchronized fun ownerStoppedStorage() {
        if (storageOwner == null) return
        // Cancelling while ordinary jobs drain keeps the owner's existing drain
        // behavior. Only a maintenance guest with no admitted jobs is torn down immediately.
        if (vm != null && ready && !storageBootRequested && !stopping.get()) {
            storageOwner = null; resumed = false; maintenanceAt = 0; maintenanceWorkloads = null
            store.update { it.copy(storageOperationPending = it.deviceId.isNotEmpty() && phoneStorage.load(it.deviceId).let { state -> state.pending != null || state.recovery != null }) }
            return
        }
        storageOwner = null; storageBootRequested = false; allowed = false; admission = false; epoch++
        halt()
    }
    private fun wantsExecution(saved: StoredState): Boolean = phoneWantsExecution(saved, visible,
        maintenance = storageOwner != null && (storageBootRequested || vm != null))

    @Synchronized fun beginApplicationUpdate(owner: Long): Boolean {
        val saved = store.load()
        if (busy.get() || allocatingVm || saved.prepareRequested || saved.resetRequest.isNotEmpty() || saved.storageOperationPending || storageOwner != null) return false
        if (updateOwner == owner && saved.applicationUpdatePending) return true
        check(updateOwner == null) { "An application update is already pending" }
        updateOwner = owner
        maintenanceAt = 0; maintenanceWorkloads = null; resumed = false; admission = false
        store.update { it.copy(applicationUpdatePending = true) }
        return true
    }

    @Synchronized fun applicationUpdateGate(owner: Long): UpdateGate {
        check(updateOwner == owner && store.load().applicationUpdatePending) { "The application update was cancelled" }
        if (stopping.get()) return UpdateGate.WaitingForTermination
        val now = SystemClock.elapsedRealtime()
        val gate = updateGate(vm != null, busy.get() || allocatingVm || (vm != null && !ready), confirmedIdle,
            controllerLeaseFresh(lastHeartbeat, now) && maintenanceAt > 0 && now - maintenanceAt < 30_000,
            if (statusAt > 0 && now - statusAt < 30_000) status?.workloadCount(systemPods) else null, maintenanceWorkloads)
        if (gate == UpdateGate.StopWorker) {
            halt(immediate = false, stopDeadline = now + 120_000)
            return UpdateGate.WaitingForTermination
        }
        return gate
    }

    @Synchronized fun cancelApplicationUpdate(owner: Long? = null) {
        if (owner != null && updateOwner != owner) return
        updateOwner = null; maintenanceAt = 0; maintenanceWorkloads = null; resumed = false
        store.update { it.copy(applicationUpdatePending = false, updateInstallVersion = 0) }
    }

    private fun workloadSnapshot(): List<GuestPod>? = when {
        confirmedIdle -> emptyList()
        SystemClock.elapsedRealtime() - statusAt < 30_000 -> status?.pods
        else -> null
    }

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
    fun ownedDiskGib(owner: String): Long = (guest.allocatedBytes(owner) + phoneStorage.allocatedBytes(owner)) / STORAGE_GIB
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
        val storage = if (saved.deviceId.isEmpty()) null else phoneStorage.load(saved.deviceId)
        val storageAvailable = if (storageOwner != null && storageBootRequested)
            runCatching { phoneStorage.vmFiles(saved.deviceId, maintenance = true) }.isSuccess
            else storage?.disabled != true && (storage?.pool == null || !phoneStorage.unavailable(saved.deviceId))
        val wants = wantsExecution(saved) && saved.resetRequest.isEmpty() && storageAvailable
        val fresh = controllerLeaseFresh(lastHeartbeat, now)
        val resourcesChanged = active != null && (active.policy.cpus != saved.policy.cpus ||
            active.policy.memoryMib != saved.policy.memoryMib || storageOwner == null && active.policy.diskGib != saved.policy.diskGib)
        val desired = wants && phone.allowed && shared.allowed && snapshot.phone.freeDiskGib >= 10 &&
            fresh && !remotePaused && failure.isEmpty() && !resourcesChanged
        reason = when {
            failure.isNotEmpty() -> failure
            saved.resetRequest.isNotEmpty() -> "Stopping the owned worker before removing its access"
            !storageAvailable -> "Selected storage is unavailable or disabled. The whole worker is stopped."
            saved.storageOperationPending && storageOwner == null -> "Finish or review the pending storage change before enabling sharing"
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
        admission = desired && ready && saved.policy.enabled && deadline == null && !saved.applicationUpdatePending && !saved.storageOperationPending
        if (active != null && !active.running && !stopping.get()) {
            if (desired && deadline == null) failure = "The isolated worker stopped unexpectedly. Check the activity and retry."
            halt()
        }
        val emergency = active != null && (!storageAvailable || workerMustForceStop(snapshot.phone.thermal, snapshot.phone.availableMemoryMib, fresh))
        if (emergency) halt()
        if (active != null && !stopping.get() && workerShouldDrain(desired, deadline, storageOwner != null, storageBootRequested)) {
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
            else if (desired && storageOwner != null && storageBootRequested) begin { prepareAndStart(saved, storageMaintenance = true) }
            else if (desired && !saved.applicationUpdatePending && !saved.storageOperationPending) begin { prepareAndStart(saved) }
        } else if (desired && deadline == null && !stopping.get()) {
            allowed = true
            stage = if (!ready) "preparing" else if (admission && (eligibleCi || eligibleServices)) "sharing" else "connecting"
        }
        if (active != null && now - lastPoll >= 10_000 && !stopping.get()) {
            lastPoll = now
            begin { updateGuest(active) }
        }
        if (saved.applicationUpdatePending && active != null && deadline == null && !stopping.get()) {
            stage = "draining"
            reason = if (!ready || busy.get()) "Preparing the worker for an application update"
                else "Waiting for current jobs before updating NodeHarbor"
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
        wake.update(workerNeedsWake(saved.policy.preventSleep, storageOwner != null || allowed && (active != null || busy.get()),
            stopping.get() && active?.shutdownOutcome == null, wake.held))
        publish(stage, reason, eligibleCi, eligibleServices, workloadSnapshot())
        if (!wants && storageOwner == null && saved.resetRequest.isEmpty() && vm == null && !busy.get() && !stopping.get()) finish()
    }

    private fun begin(action: () -> Unit) {
        if (!busy.compareAndSet(false, true)) return
        val operationEpoch = epoch
        work.execute { try { action() } catch (error: Exception) {
            synchronized(this) { if (epoch == operationEpoch && (allowed || store.load().resetRequest.isNotEmpty())) fail(error) }
        }
            finally { busy.set(false) } }
    }
    private fun prepareAndStart(saved: StoredState, storageMaintenance: Boolean = false) {
        check(saved.deviceId.isNotEmpty()) { "Connect this phone to your fleet first" }
        stage = "preparing"
        val operationEpoch = epoch
        val rootPolicy = if (storageMaintenance || phoneStorage.load(saved.deviceId).pool != null)
            saved.policy.copy(diskGib = guest.receipt(saved.deviceId)?.diskGib ?: 15) else saved.policy
        val receipt = guest.prepare(saved.deviceId, rootPolicy, { epoch == operationEpoch && attached && allowed && wantsExecution(store.load()) }) {
            if (epoch == operationEpoch) { reason = it; record(it) }
        }
        check(allowed && attached && wantsExecution(store.load())) { "Worker preparation was stopped" }
        val preflight = phoneDecision(saved.policy, phoneSnapshot(context, true).phone, saved.policy.memoryMib)
        check(preflight.allowed) { preflight.reason }
        val bootDeadline = SystemClock.elapsedRealtime() + 1_200_000
        allocatingVm = true
        val session = try {
            VmSession.start(context, saved.deviceId, saved.policy, guest, { attached && allowed && epoch == operationEpoch },
                { if (epoch == operationEpoch) record(it) }, storage = phoneStorage, storageMaintenance = storageMaintenance, created = { created ->
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
        if (storageMaintenance) return
        if (!first.configured || first.configurationRevision != GUEST_CONTROL_REVISION) {
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
        val storage = phoneStorage.load(store.load().deviceId).pool
        ready = workerReady(observed, storage?.generation ?: 0, storage?.poolId ?: "")
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
            val result = controller.request("/heartbeat", JSONObject().put("state", if (stage == "forced-stop") "paused" else stage).put("reason", reason.take(2048))
                .put("resources", allocation.sharedJson().getJSONObject("resources"))
                .put("allowCi", saved.policy.allowCi).put("allowServices", saved.policy.allowServices)
                .put("permitted", admission).put("storageGeneration", phoneStorage.load(saved.deviceId).generation)) as JSONObject
            synchronized(this) {
                if (operationEpoch != epoch || !attached || stopping.get()) return
            remotePaused = result.getBoolean("remotePaused")
            eligibleCi = result.getBoolean("eligibleCi"); eligibleServices = result.getBoolean("eligibleServices")
            lastHeartbeat = SystemClock.elapsedRealtime()
            }
            val maintenanceOwner = synchronized(this) { updateOwner }
            if (saved.applicationUpdatePending && maintenanceOwner != null && deadline == null && ready) {
                val resultMaintenance = controller.request("/device/maintenance", JSONObject()) as JSONObject
                val count = resultMaintenance.getLong("workloads").also { require(it in 0..Int.MAX_VALUE) }.toInt()
                val pods = resultMaintenance.getJSONArray("systemPodUids")
                require(pods.length() <= 4096)
                val observed = (0 until pods.length()).map { pods.getString(it).also { id -> require(id.length in 1..128) } }.toSet()
                synchronized(this) {
                    if (operationEpoch != epoch || !attached || stopping.get() || updateOwner != maintenanceOwner) return
                    systemPods = (systemPods + observed).also { require(it.size <= 4096) }
                    maintenanceWorkloads = count; maintenanceAt = SystemClock.elapsedRealtime()
                }
            } else if (deadline != null && !drained) {
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
            publish(stage, reason, false, false, workloadSnapshot())
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
        publish(stage, reason, false, false, workloadSnapshot())
    }
    private fun replace(saved: StoredState) {
        check(vm == null && !stopping.get()) { "Wait for Android to confirm worker shutdown" }
        val reset = client().request("/device/reset", JSONObject().put("requestId", saved.resetRequest)) as JSONObject
        check(reset.getBoolean("complete")) { "The fleet is still removing the previous worker's access" }
        val storage = phoneStorage.load(saved.deviceId)
        if (saved.storageDeleteRequested || storage.pool != null || storage.pending != null)
            phoneStorage.removeAll(saved.deviceId, confirmedStopped = true, resetConfirmed = true, disabled = true)
        if (guest.receipt(saved.deviceId) != null) guest.remove(saved.deviceId, stopped = true, reset = true)
        store.update { it.copy(workerInstalled = false, workerGeneration = "", resetRequest = "", prepareRequested = false,
            policy = it.policy.copy(enabled = false), userStopped = true, storageDeleteRequested = false, storageOperationPending = false) }
        record("The previous worker was removed. Your enrollment and sharing rules were preserved.")
    }
    @Synchronized override fun close() {
        attached = false; allowed = false; admission = false
        storageOwner = null; storageBootRequested = false
        epoch++
        halt()
        guard.shutdownNow(); network.shutdownNow(); lease.shutdownNow(); work.shutdownNow()
        if (vm == null) wake.update(false)
    }
}
