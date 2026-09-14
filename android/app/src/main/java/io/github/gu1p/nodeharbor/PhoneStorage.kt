package io.github.gu1p.nodeharbor

import android.content.Context
import android.os.SystemClock
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/** Owner-controlled storage work runs independently of HTTP, lease renewal, and emergency teardown. */
class PhoneStorage(private val context: Context, private val store: PrivateStore, private val supervisor: PhoneSupervisor,
    private val startService: () -> Unit, private val client: () -> ControllerClient, private val publish: (StorageUiState) -> Unit,
    private val record: (String) -> Unit, private val reportError: (String) -> Unit) {
    private val owned = OwnedPhoneStorage(context)
    private val executor = Executors.newSingleThreadExecutor { Thread(it, "nodeharbor-storage-work").apply { isDaemon = true } }
    private val observer = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-storage-state").apply { isDaemon = true } }
    private val working = AtomicBoolean(false)
    private val generation = AtomicLong(0)
    @Volatile private var automaticToken: Long? = null
    @Volatile private var ui = StorageUiState()
    private var missingSince: Long? = null
    init {
        observer.scheduleWithFixedDelay({ runCatching { refresh() }.onFailure { update { state -> state.copy(message = "Storage could not be inspected. Original files have been preserved.") } } },
            0, 5, TimeUnit.SECONDS)
    }
    @Synchronized private fun update(change: (StorageUiState) -> StorageUiState) { ui = change(ui); publish(ui) }
    fun refresh() {
        val saved = store.load()
        val locations = owned.locations.snapshot()
        val state = saved.deviceId.takeIf { it.isNotEmpty() }?.let(owned::load)
        val missing = state?.pool != null && owned.unavailable(saved.deviceId)
        val absent = state?.pool?.disks.orEmpty().map { it.location }.distinct().filter { id -> locations.none { it.id == id } }
            .map { PhoneStorageLocation(it, "Unavailable selected storage", "", 0, false, true) }
        update { previous -> previous.copy(locations = locations + absent, pool = state?.pool, missing = missing,
            pending = state?.pending != null || state?.recovery != null, retained = (state?.retained?.size ?: 0) + (state?.backups?.size ?: 0),
            automaticRecovery = state?.automaticRecovery == true, busy = working.get(), message = when {
                working.get() -> previous.message
                saved.resetRequest.isNotEmpty() -> "Waiting for worker shutdown and fleet access removal before deleting storage."
                state?.recovery != null -> "Whole-pool recovery is pending. The original reset request and selected replacement location are retained."
                state?.pending != null -> "A storage change is pending. Original files and any verified backup are retained. Resume it when ready."
                missing -> "Selected storage is unavailable. The whole worker is stopped."
                state?.disabled == true -> "Worker storage is disabled. Review new locations before preparing the worker."
                state?.pool != null -> "Owned worker storage is configured. All selected disks must be available before sharing."
                saved.deviceId.isNotEmpty() -> "The current worker uses its private system disk. Choose a pool to move workload data to selected app storage."
                else -> "Connect to your fleet before allocating worker storage."
            }, excluded = state?.retained.orEmpty().filter { old -> state?.pool != null && state.pool.disks.none { it.location == old.location } }
                .distinctBy { it.location }) }
        val now = SystemClock.elapsedRealtime()
        if (!missing && state?.recovery == null || saved.userStopped || !saved.policy.enabled || state?.automaticRecovery != true) missingSince = null
        else if (missingSince == null) missingSince = now
        val replacement = state?.pool?.let { pool ->
            val credits = owned.recoveryCredits(saved.deviceId).toMutableMap()
            locations.find { it.id == "internal" && it.available }?.let { internal ->
                credits[internal.filesystem] = (credits[internal.filesystem] ?: 0) + OwnedGuest(context).allocatedBytes(saved.deviceId)
            }
            replacementStorage(pool, locations, credits)
        }
        if (!working.get() && missing && state?.automaticRecovery == true && state.recovery == null && replacement == null)
            update { it.copy(message = "Selected storage is unavailable. Reconnect it: remaining selected disks need at least 15 GiB of capacity and the 10 GiB free-space reserve.") }
        if (!working.get() && saved.resetRequest.isEmpty() && !saved.applicationUpdatePending && (supervisor.visible || saved.policy.background) &&
            missingSince != null && storageRecoveryDue(state?.automaticRecovery == true, saved.userStopped, saved.policy.enabled,
                replacement != null || state?.recovery != null, missingSince!!, now) &&
            phoneDecision(saved.policy, phoneSnapshot(context, true).phone, saved.policy.memoryMib).allowed) {
            run(automatic = true) { token, owner -> recover(token, owner, replacement) }
        }
    }
    fun review(disks: List<PhoneStorageDisk>) {
        check(!working.get()) { "Wait for the current storage operation" }
        val owner = store.load().deviceId
        check(owner.isNotEmpty()) { "Connect this phone to your fleet first" }
        val review = owned.review(owner, disks)
        update { it.copy(review = review) }
    }
    fun recoveryPreference(enabled: Boolean) {
        val saved = store.load()
        check(saved.deviceId.isNotEmpty() && !working.get()) { "Wait for current storage maintenance" }
        owned.recoveryPreference(saved.deviceId, enabled)
        if (!enabled && automaticToken != null) cancel()
        refresh()
    }
    fun cancel() {
        if (!working.get()) return
        generation.incrementAndGet()
        supervisor.ownerStoppedStorage()
        update { it.copy(message = "Stopping storage work. Its journal, original disks, and verified backup are preserved.") }
    }
    fun action(action: String) {
        when (action) {
            "dismiss-review" -> update { it.copy(review = null) }
            "cancel" -> cancel()
            "apply" -> {
                val review = checkNotNull(ui.review) { "Review storage changes first" }
                run { token, owner -> change(token, owner, review) }
            }
            "resume" -> run { token, owner -> if (owned.load(owner).recovery != null) recover(token, owner, null) else change(token, owner, null) }
            "cleanup" -> run { token, owner -> cleanup(token, owner) }
            else -> error("Unsupported storage action")
        }
    }
    private fun active(token: Long): Boolean = generation.get() == token && (automaticToken != token || runCatching {
        val saved = store.load()
        saved.policy.enabled && !saved.userStopped && (supervisor.visible || saved.policy.background) &&
            saved.deviceId.isNotEmpty() && owned.load(saved.deviceId).automaticRecovery
    }.getOrDefault(false))
    private fun requireActive(token: Long) { check(active(token)) { "Storage work was stopped by the owner; its files have been preserved" } }
    private fun progress(message: String) { update { it.copy(message = message) } }
    private fun run(automatic: Boolean = false, work: (Long, String) -> Unit) {
        check(working.compareAndSet(false, true)) { "Storage work is already running" }
        val token = generation.incrementAndGet()
        automaticToken = if (automatic) token else null
        update { it.copy(busy = true, review = null, message = "Preparing storage maintenance") }
        executor.execute {
            var owner = ""
            try {
                owner = store.load().deviceId
                requireActive(token)
                check(owner.isNotEmpty()) { "Connect this phone to your fleet first" }
                check(!store.load().applicationUpdatePending) { "Finish or cancel the application update before changing storage" }
                while (true) {
                    requireActive(token)
                    if (supervisor.beginStorageMaintenance(token)) break
                    check(!store.load().applicationUpdatePending) { "An application update is pending" }
                    progress("Waiting for current worker preparation to finish")
                    Thread.sleep(1000)
                }
                requireActive(token)
                startService()
                waitUntil(token, SystemClock.elapsedRealtime() + 30_000) { supervisor.supervised }
                waitForIdle(token)
                requireActive(token)
                work(token, owner)
                requireActive(token)
                supervisor.endStorageMaintenance(token)
                record("Worker storage operation completed. Enrollment and current sharing preferences were preserved.")
            } catch (error: Exception) {
                supervisor.ownerStoppedStorage()
                if (active(token)) reportError(error.message ?: "Storage maintenance failed; its journal and original data were preserved")
            } finally {
                // A cancelled review with no disk journal must not leave an invisible startup hold.
                runCatching {
                    if (owner.isNotEmpty() && supervisor.confirmedIdle && owned.load(owner).let { it.pending == null && it.recovery == null } && store.load().resetRequest.isEmpty())
                        store.update { it.copy(storageOperationPending = false) }
                }
                automaticToken = null
                working.set(false)
                runCatching { refresh() }
            }
        }
    }
    private fun waitUntil(token: Long, deadline: Long, complete: () -> Boolean) {
        while (true) {
            requireActive(token)
            if (complete()) return
            check(SystemClock.elapsedRealtime() < deadline) { "Storage maintenance exceeded its deadline. Original data and its journal were preserved." }
            Thread.sleep(1000)
        }
    }
    private fun waitForIdle(token: Long) {
        while (true) {
            requireActive(token)
            when (supervisor.storageMaintenanceGate(token)) {
                UpdateGate.Ready -> return
                UpdateGate.WaitingForController -> progress("Waiting for a fresh controller maintenance response")
                UpdateGate.WaitingForJobs -> progress("Draining current work within your existing deadline before storage maintenance")
                UpdateGate.WaitingForTermination -> progress("Waiting for Android to confirm worker shutdown")
                else -> progress("Preparing the worker for storage maintenance")
            }
            Thread.sleep(1000)
        }
    }
    private fun command(token: Long, owner: String, operation: String, action: String, previousPoolId: String?,
                        pool: PhoneStoragePool? = null, previous: PhoneStoragePool? = null, restore: Boolean = false): JSONObject {
        requireActive(token)
        val policy = store.load().policy
        val preflight = phoneDecision(policy, phoneSnapshot(context, true).phone, policy.memoryMib)
        check(preflight.allowed) { preflight.reason }
        supervisor.requestStorageBoot(token)
        var session: VmSession? = null
        progress("Starting the owned guest for storage maintenance. Boot may take several minutes.")
        waitUntil(token, SystemClock.elapsedRealtime() + 1_200_000) { supervisor.storageSession(token).also { session = it } != null }
        val vm = checkNotNull(session)
        val request = JSONObject().put("deviceId", owner).put("operation", operation).put("action", action)
            .put("previousPoolId", previousPoolId ?: JSONObject.NULL)
        if (action == "apply") request.put("pool", checkNotNull(pool).guestRequest(previous)).put("restore", restore)
        vm.storage(request)
        progress(when (action) { "backup" -> "Creating and verifying the whole-pool backup inside the guest"; "cleanup" -> "Removing a verified retained backup"; else -> "Applying and verifying the reviewed storage pool inside the guest" })
        var result: JSONObject? = null
        waitUntil(token, SystemClock.elapsedRealtime() + 1_200_000) {
            supervisor.storageSession(token) // Reject a cancelled or failed VM session.
            val observed = vm.status()
            check(observed.storageError.isEmpty()) { observed.storageError }
            observed.storageResult?.takeIf { it.optString("operation") == operation && it.optString("action") == action && it.optBoolean("ok") }
                .also { result = it } != null
        }
        supervisor.stopStorageVm(token)
        progress("Storage verified. Waiting for confirmed Android process death before changing disk receipts.")
        waitUntil(token, SystemClock.elapsedRealtime() + 126_000) { supervisor.confirmedIdle }
        return checkNotNull(result)
    }
    private fun change(token: Long, owner: String, review: PhoneStorageReview?) {
        val change = if (review == null) checkNotNull(owned.load(owner).pending) { "No storage change is pending" }
            else owned.begin(owner, owned.review(owner, review.target).also {
                check(it.generation == review.generation && it.requiresBackup == review.requiresBackup && it.requiredBytes == review.requiredBytes) { "Storage changed after review; review it again" }
            }, supervisor.confirmedIdle)
        if (change.requiresBackup && change.phase == "planned") {
            val result = command(token, owner, change.id, "backup", change.previous?.poolId)
            check(result.getBoolean("verified") && result.getString("poolId") == change.previous?.poolId)
            requireActive(token)
            owned.markBackedUp(owner, change.id, supervisor.confirmedIdle)
        }
        if (owned.load(owner).pending?.phase != "verified") {
            owned.prepare(owner, supervisor.confirmedIdle, { active(token) }, ::progress)
            val result = command(token, owner, change.id, "apply", change.previous?.poolId, change.target, change.previous, change.requiresBackup)
            check(result.getLong("capacityBytes") >= change.target.gib * STORAGE_GIB * 9 / 10) { "The guest did not verify the reviewed storage capacity" }
            requireActive(token)
            owned.markVerified(owner, change.id, result.getLong("generation"), result.getString("poolId"), supervisor.confirmedIdle)
        }
        requireActive(token)
        owned.commit(owner, supervisor.confirmedIdle)
        store.update { it.copy(policy = it.policy.copy(diskGib = change.target.gib)) }
    }
    private fun cleanup(token: Long, owner: String) {
        val backups = owned.load(owner).backups
        for (backup in backups) {
            val result = command(token, owner, backup.operation, "cleanup", backup.poolId)
            check(result.getBoolean("cleaned"))
            owned.forgetBackups(owner, listOf(backup), supervisor.confirmedIdle)
        }
        requireActive(token)
        owned.cleanupRetained(owner, supervisor.confirmedIdle)
    }
    private fun recover(token: Long, owner: String, replacement: List<PhoneStorageDisk>?) {
        requireActive(token)
        val recovery = owned.load(owner).recovery ?: owned.beginRecovery(owner, checkNotNull(replacement), supervisor.confirmedIdle)
        if (recovery.phase == "resetting") {
            progress("Removing the old worker's fleet access before authorized whole-pool recovery")
            val reset = client().request("/device/reset", JSONObject().put("requestId", recovery.operation)) as JSONObject
            check(reset.getBoolean("complete")) { "The fleet is still removing the old worker's access" }
            requireActive(token)
            owned.removeAll(owner, supervisor.confirmedIdle, true, true, preserveRecovery = true) { active(token) }
            requireActive(token)
            val guest = OwnedGuest(context)
            if (guest.receipt(owner) != null) guest.remove(owner, supervisor.confirmedIdle, true)
            requireActive(token)
            store.update { it.copy(workerInstalled = false, workerGeneration = "", prepareRequested = false) }
            owned.recoveryRebuilding(owner, recovery.operation, supervisor.confirmedIdle)
        }
        val state = owned.load(owner)
        if (state.pending != null) change(token, owner, null)
        else if (state.pool == null) change(token, owner, owned.review(owner, recovery.replacement))
        requireActive(token)
        owned.finishRecovery(owner, recovery.operation, supervisor.confirmedIdle)
        missingSince = null
    }
}
