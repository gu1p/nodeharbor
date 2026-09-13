package io.github.gu1p.nodeharbor

import org.json.JSONObject
import java.util.UUID

/** Called only with a credential successfully decrypted by Android Keystore. */
fun recoverEnrollment(saved: StoredState, credential: String?): StoredState {
    if (credential == null) {
        check(saved.deviceId.isEmpty()) { "The saved fleet credential is unavailable; the original enrollment has been preserved" }
        return saved
    }
    val data = JSONObject(credential)
    val id = data.getString("deviceId")
    require(UUID.fromString(id).toString() == id) { "The saved enrollment identity is invalid" }
    val address = controllerAddress(data.getString("address"))
    val token = data.getString("token")
    require(token.length in 32..8192 && token.none { it.isWhitespace() }) { "The saved enrollment credential is invalid" }
    if (saved.deviceId.isNotEmpty()) {
        check(saved.deviceId == id && saved.controllerUrl == address) { "The saved enrollment does not match this fleet; its original files have been preserved" }
        return saved
    }
    check(!saved.workerInstalled && saved.workerGeneration.isEmpty() && saved.controllerUrl.isEmpty()) {
        "The saved worker needs recovery before reconnecting; its files have been preserved"
    }
    return saved.copy(deviceId = id, controllerUrl = address, policy = saved.policy.copy(enabled = false), prepareRequested = false)
}
