package io.github.gu1p.nodeharbor

import org.json.JSONObject
import org.json.JSONTokener
import java.util.UUID

enum class GuestCommand(val wire: String) { Status("status"), Lease("lease"), Configure("configure"), Poweroff("poweroff"), Storage("storage") }

fun guestRequest(id: Long, owner: String, command: GuestCommand, bootstrap: JSONObject? = null): ByteArray {
    require(id in 0 until (1L shl 53) && UUID.fromString(owner).toString() == owner)
    require((command in setOf(GuestCommand.Configure, GuestCommand.Storage)) == (bootstrap != null)) { "This owner command requires its fixed payload" }
    if (bootstrap != null) require(bootstrap.getString("deviceId") == owner) { "The bootstrap grant belongs to a different phone" }
    val request = JSONObject().put("id", id).put("deviceId", owner).put("command", command.wire)
    if (bootstrap != null) request.put(if (command == GuestCommand.Storage) "storage" else "bootstrap", bootstrap)
    return request.toString().toByteArray(Charsets.UTF_8).also { require(it.size <= 1024 * 1024) { "The owner command exceeds its supported size" } }
}

fun guestResponse(bytes: ByteArray, id: Long, owner: String): JSONObject {
    require(bytes.size in 2..1024 * 1024) { "Invalid guest response size" }
    var quoted = false
    var escaped = false
    var depth = 0
    for (byte in bytes) {
        val char = (byte.toInt() and 255).toChar()
        if (quoted) {
            if (escaped) escaped = false
            else if (char == '\\') escaped = true
            else if (char == '"') quoted = false
        } else when (char) {
            '"' -> quoted = true
            '{', '[' -> { depth++; require(depth <= 16) { "The guest response is too deeply nested" } }
            '}', ']' -> { depth--; require(depth >= 0) { "Invalid guest response structure" } }
        }
    }
    require(!quoted && depth == 0) { "Invalid guest response structure" }
    try {
        val parser = JSONTokener(bytes.toString(Charsets.UTF_8))
        val response = parser.nextValue()
        require(response is JSONObject && parser.nextClean() == '\u0000') { "The guest returned an invalid owner response" }
        require(response.getLong("id") == id && response.getString("deviceId") == owner) { "The guest response does not match this owner request" }
        check(response.getBoolean("ok")) { "The guest could not complete the owner command" }
        return response
    } catch (error: org.json.JSONException) { throw IllegalArgumentException("The guest returned an invalid owner response", error) }
}
