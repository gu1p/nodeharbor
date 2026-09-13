package io.github.gu1p.nodeharbor

import java.io.InputStream
import java.net.URI
import javax.net.ssl.HttpsURLConnection
import org.json.JSONObject
import org.json.JSONTokener

fun controllerAddress(address: String): String {
    try {
        val uri = URI(address.trim())
        require(uri.scheme == "https" && !uri.host.isNullOrEmpty() && uri.rawUserInfo == null &&
            uri.rawQuery == null && uri.rawFragment == null && uri.port in -1..65535 && uri.port != 0 &&
            uri.path.split('/').none { it == "." || it == ".." } && '\\' !in uri.path)
        return uri.toASCIIString().trimEnd('/')
    } catch (error: Exception) {
        throw IllegalArgumentException("Use a valid HTTPS controller address without credentials, query parameters, or fragments", error)
    }
}

class ControllerFailure(val status: Int) : IllegalStateException(when (status) {
    401, 403 -> "The fleet rejected this enrollment or its saved credential"
    in 300..399 -> "The fleet controller redirected the request; check its address"
    409 -> "The fleet could not perform that action in its current state"
    else -> "The fleet controller could not complete the request (HTTP $status)"
})

fun checkedControllerResponse(status: Int, body: InputStream): String {
    if (status !in 200..299) throw ControllerFailure(status)
    val bytes = body.readNBytes(1024 * 1024 + 1)
    require(bytes.size <= 1024 * 1024) { "The fleet response exceeds its supported size" }
    return bytes.toString(Charsets.UTF_8)
}

/** All calls use ordinary Android networking and the existing system VPN policy. */
class ControllerClient(address: String, private val token: String? = null) {
    val address = controllerAddress(address)
    fun request(path: String, payload: JSONObject? = null): Any {
        require(path in setOf("/enroll", "/device/fleet", "/heartbeat", "/device/bootstrap", "/device/resume", "/device/drain", "/device/reset"))
        check(path == "/enroll" || !token.isNullOrEmpty()) { "Connect this phone to your fleet first" }
        val connection = URI("$address/api/v1$path").toURL().openConnection() as HttpsURLConnection
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 20_000
            connection.readTimeout = 20_000
            connection.setRequestProperty("Accept", "application/json")
            if (path != "/enroll") connection.setRequestProperty("Authorization", "Bearer $token")
            connection.requestMethod = if (payload == null) "GET" else "POST"
            if (payload != null) {
                val bytes = payload.toString().toByteArray(Charsets.UTF_8)
                require(bytes.size <= 1024 * 1024)
                connection.doOutput = true
                connection.setFixedLengthStreamingMode(bytes.size)
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(bytes) }
            }
            val status = connection.responseCode
            if (status !in 200..299) throw ControllerFailure(status)
            val body = connection.inputStream.use { checkedControllerResponse(status, it) }
            return JSONTokener(body).nextValue()
        } finally { connection.disconnect() }
    }
}
