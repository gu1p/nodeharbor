package io.github.gu1p.nodeharbor

import org.json.JSONObject
import java.net.URI
import java.util.Base64

data class AppUpdateCandidate(val version: String, val versionCode: Int, val commit: String, val url: String, val signature: String)
data class AppUpdateStatus(val enabled: Boolean = true, val phase: String = "idle",
    val message: String = "Updates are checked while NodeHarbor is running",
    val availableVersion: String? = null, val downloaded: Long = 0, val total: Long? = null)

fun automaticReleaseUpdates(version: String, dirtySource: Boolean): Boolean =
    !dirtySource && runCatching { appVersionCode(version) }.isSuccess

fun appVersionCode(version: String): Int {
    require(Regex("(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)").matches(version)) { "The update requires a numeric release version" }
    val parts = version.split('.').map { it.toLongOrNull() ?: throw IllegalArgumentException("Invalid update version") }
    require(parts[0] <= 21 && parts[1] < 100 && parts[2] < 1_000_000) { "Unsupported update version" }
    val code = parts[0] * 100_000_000 + parts[1] * 1_000_000 + parts[2]
    require(code in 1..2_100_000_000) { "Unsupported update version" }
    return code.toInt()
}

fun requireUpdateHttps(value: String): URI {
    val uri = URI(value)
    require(value.length <= 8192 && uri.scheme == "https" && !uri.host.isNullOrBlank() &&
        uri.userInfo == null && uri.fragment == null && (uri.port == -1 || uri.port in 1..65535)) { "Updates require an HTTPS address without embedded credentials" }
    return uri
}

fun parseAppUpdate(text: String, currentVersion: String): AppUpdateCandidate? {
    require(text.length <= 1024 * 1024) { "The update response exceeds its supported size" }
    try {
        val data = JSONObject(text)
        val version = data.getString("version")
        val code = appVersionCode(version)
        val current = runCatching { appVersionCode(currentVersion) }.getOrDefault(0)
        if (code <= current) return null
        val platform = data.getJSONObject("platforms").optJSONObject("android-aarch64-apk") ?: return null
        val url = platform.getString("url").also(::requireUpdateHttps)
        val commit = data.getString("commit").also { require(Regex("[0-9a-f]{40}").matches(it)) }
        val encoded = platform.getString("signature")
        require(encoded.length <= 12000)
        val signature = Base64.getDecoder().decode(encoded).toString(Charsets.UTF_8)
        require(signature.length in 1..8192)
        return AppUpdateCandidate(version, code, commit, url, signature)
    } catch (error: Exception) {
        throw IllegalArgumentException("The Android update metadata is invalid", error)
    }
}
