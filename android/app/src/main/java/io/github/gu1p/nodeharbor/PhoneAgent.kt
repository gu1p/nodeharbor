package io.github.gu1p.nodeharbor

import android.app.Application
import android.content.Context
import android.os.Build
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.util.UUID

class NodeHarborApplication : Application() {
    val agent by lazy { PhoneAgent(this) }
}

class PhoneAgent(private val context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val operation = Mutex()
    private val store = PrivateStore(context.noBackupFilesDir.resolve("nodeharbor"))
    private val mutable = MutableStateFlow(UiState(phoneName = Build.MODEL.take(120),
        phoneDescription = "Android ${Build.VERSION.RELEASE} · ARM64"))
    val state = mutable.asStateFlow()
    val supervisor = PhoneSupervisor(context, store, ::client, { status, reason, ci, services ->
        val saved = store.load()
        mutable.update { it.copy(state = status, reason = reason, policy = saved.policy,
            workerInstalled = saved.workerInstalled, eligibleCi = ci, eligibleServices = services) }
    }, ::record, ::showError)

    fun visibility(visible: Boolean) { supervisor.visible = visible }
    fun recoverInBackground(): Boolean = store.load().let { shouldAutoStart(it.policy, StartCause.Recovery, it.policy.enabled, it.userStopped, false) }
    fun startAutomatically(cause: StartCause) {
        try {
            val saved = recoverSavedEnrollment()
            if (shouldAutoStart(saved.policy, cause, saved.policy.enabled, saved.userStopped, false)) startService()
        } catch (error: Exception) { showError(error.message ?: "Automatic worker startup was unavailable") }
    }
    private fun startService() {
        try { WorkerService.start(context) }
        catch (_: IllegalStateException) {
            stopOwner()
            error("Android did not allow background startup. Open NodeHarbor and enable sharing again.")
        }
    }
    /** Never queue an owner stop behind a slow enrollment, fleet request or setup. */
    fun stopOwner() {
        store.update { it.copy(policy = it.policy.copy(enabled = false), userStopped = true, prepareRequested = false) }
        record("Sharing switched off; current work has only its remaining drain allowance")
        refresh()
    }

    fun refresh() = task(busy = false) { refreshState() }
    private fun recoverSavedEnrollment(): StoredState {
        val saved = store.load()
        val recovered = recoverEnrollment(saved, store.token())
        if (saved != recovered) store.update { recovered }
        return recovered
    }
    private fun refreshState() {
        val saved = recoverSavedEnrollment()
        val runtimeReady = Build.SUPPORTED_ABIS.contains("arm64-v8a") &&
            java.io.File(context.applicationInfo.nativeLibraryDir, "libqemu-system-aarch64.so").isFile
        val snapshot = phoneSnapshot(context, runtimeReady, supervisor.ownedDiskGib(saved.deviceId))
        mutable.update { it.copy(policy = saved.policy, permissions = snapshot.permissions,
            enrolled = saved.deviceId.isNotEmpty(), controllerUrl = saved.controllerUrl,
            workerInstalled = saved.workerInstalled,
            runtimeReason = if (runtimeReady) "" else "This installation requires the packaged ARM64 VM runtime",
            reason = workerDisplayReason(supervisor.supervised, saved.policy.enabled, it.reason, it.state)) }
    }

    fun enroll(address: String, code: String) = task {
        check(recoverSavedEnrollment().deviceId.isEmpty()) { "This phone is already enrolled; its enrollment has been preserved" }
        check(store.token() == null) { "A saved enrollment needs recovery before enrolling again" }
        require(code.trim().length in 1..4096) { "Enter the enrollment code supplied by your fleet administrator" }
        val client = ControllerClient(address)
        val response = client.request("/enroll", JSONObject().put("code", code.trim()).put("name", Build.MODEL.take(120))
            .put("platform", "android").put("architecture", "arm64")) as JSONObject
        val id = response.getString("deviceId").also(UUID::fromString)
        val token = response.getString("token")
        require(token.length in 32..8192 && token.none { it.isWhitespace() }) { "The fleet returned an invalid enrollment credential" }
        // Bind the encrypted credential to its origin and identity, including if
        // Android stops the process between these two atomic writes.
        store.saveToken(JSONObject().put("deviceId", id).put("address", client.address).put("token", token).toString())
        store.update { it.copy(deviceId = id, controllerUrl = client.address, policy = it.policy.copy(enabled = false)) }
        record("Connected to your fleet. Sharing remains off.")
        refreshState()
    }

    private fun client(): ControllerClient {
        val saved = store.load()
        val encrypted = checkNotNull(store.token()) { "Connect this phone to your fleet first" }
        val credential = JSONObject(encrypted)
        check(credential.getString("deviceId") == saved.deviceId && credential.getString("address") == saved.controllerUrl) {
            "The saved enrollment does not match this fleet; its original files have been preserved"
        }
        return ControllerClient(saved.controllerUrl, credential.getString("token"))
    }

    fun action(action: UiAction) {
        if (action == UiAction.Pause || action == UiAction.Stop) { stopOwner(); return }
        task {
        when (action) {
            UiAction.ContinuousSetup -> {
                store.update { it.copy(policy = it.policy.continuous()) }
                record("Continuous sharing preferences saved. Review the phone setup permissions.")
            }
            is UiAction.SavePolicy -> {
                val saved = store.load()
                val snapshot = phoneSnapshot(context, true, supervisor.ownedDiskGib(saved.deviceId))
                NativeRules.validate(action.policy, snapshot.host)?.let { throw IllegalArgumentException(it) }
                store.update { it.copy(policy = action.policy.copy(enabled = it.policy.enabled)) }
                record("Sharing rules saved")
            }
            UiAction.RefreshFleet -> if (store.load().deviceId.isNotEmpty()) {
                val response = client().request("/device/fleet") as JSONArray
                val fleet = (0 until response.length()).map { index -> response.getJSONObject(index).let {
                    FleetDevice(it.getString("name").take(120), it.getString("platform").take(32),
                        it.getString("state").take(32), it.optString("reason").take(2048))
                } }
                mutable.update { it.copy(fleet = fleet) }
            }
            UiAction.Prepare, UiAction.Resume -> {
                check(!supervisor.lifecycleStopping) { "Wait for Android to confirm worker shutdown" }
                val saved = recoverSavedEnrollment()
                check(saved.deviceId.isNotEmpty()) { "Connect this phone to your fleet first" }
                val snapshot = phoneSnapshot(context, true, supervisor.ownedDiskGib(saved.deviceId))
                NativeRules.validate(saved.policy, snapshot.host)?.let { throw IllegalArgumentException(it) }
                check(saved.resetRequest.isEmpty()) { "Finish removing the previous worker before preparing another" }
                supervisor.retry()
                store.update { it.copy(userStopped = false, prepareRequested = action == UiAction.Prepare,
                    policy = it.policy.copy(enabled = action == UiAction.Resume)) }
                startService()
            }
            UiAction.Replace -> {
                check(!supervisor.lifecycleStopping) { "Wait for Android to confirm worker shutdown" }
                store.update { it.copy(policy = it.policy.copy(enabled = false), userStopped = true, prepareRequested = false,
                    resetRequest = it.resetRequest.ifEmpty { UUID.randomUUID().toString() }) }
                supervisor.retry()
                startService()
            }
            else -> Unit // Permission and clipboard actions belong to the activity.
        }
        refreshState()
        }
    }

    fun showError(message: String) { mutable.update { it.copy(error = message) } }
    private fun record(message: String) { mutable.update { it.copy(activity = (it.activity + message.take(2048)).takeLast(500)) } }
    private fun task(busy: Boolean = true, work: () -> Unit) = scope.launch {
        operation.withLock {
            if (busy) mutable.update { it.copy(busy = true, error = "") }
            try { work() }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                val message = when (error) {
                    is IOException -> "The request could not reach your fleet. Check the connection and controller address."
                    is org.json.JSONException -> "The fleet or saved enrollment returned an invalid response"
                    else -> error.message ?: "NodeHarbor could not complete this operation"
                }
                showError(message)
            } finally { if (busy) mutable.update { it.copy(busy = false) } }
        }
    }
}
