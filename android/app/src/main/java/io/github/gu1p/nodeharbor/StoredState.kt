package io.github.gu1p.nodeharbor

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

data class StoredState(
    val policy: PhonePolicy = PhonePolicy(),
    val deviceId: String = "",
    val controllerUrl: String = "",
    val installedVersion: Int = BuildConfig.VERSION_CODE,
    val userStopped: Boolean = false,
    val prepareRequested: Boolean = false,
    val workerInstalled: Boolean = false,
    val drainingSince: Long? = null,
    val workerGeneration: String = "",
    val drainDeadlineMillis: Long? = null,
    val resetRequest: String = "",
) {
    fun encode(): String = JSONObject().put("formatVersion", 1)
        .put("shared", policy.sharedJson()).put("phone", policy.phoneJson())
        .put("deviceId", deviceId).put("controllerUrl", controllerUrl)
        .put("installedVersion", installedVersion).put("userStopped", userStopped)
        .put("prepareRequested", prepareRequested).put("workerInstalled", workerInstalled)
        .put("drainingSince", drainingSince ?: JSONObject.NULL)
        .put("workerGeneration", workerGeneration)
        .put("drainDeadlineMillis", drainDeadlineMillis ?: JSONObject.NULL).put("resetRequest", resetRequest).toString()

    companion object {
        fun decode(text: String, installedVersion: Int): StoredState {
            try {
                require(text.length <= 1024 * 1024) { "Saved settings exceed their supported size" }
                val data = JSONObject(text)
                require(data.getInt("formatVersion") == 1) { "These settings require a newer NodeHarbor app" }
                val id = data.getString("deviceId")
                if (id.isNotEmpty()) UUID.fromString(id)
                val shared = data.getJSONObject("shared")
                val phone = data.getJSONObject("phone")
                val resources = shared.getJSONObject("resources")
                val windows = shared.getJSONArray("schedule")
                val changed = data.getInt("installedVersion") != installedVersion
                return StoredState(
                    policy = PhonePolicy(
                        enabled = !changed && shared.getBoolean("enabled"),
                        cpus = resources.getInt("cpus"), memoryMib = resources.getInt("memoryMib"), diskGib = resources.getInt("diskGib"),
                        allowBattery = shared.getBoolean("allowBattery"), minBatteryPercent = shared.getInt("minBatteryPercent"),
                        allowCi = shared.getBoolean("allowCi"), allowServices = shared.getBoolean("allowServices"),
                        drainSeconds = shared.getInt("drainSeconds"), scheduleEnabled = shared.getBoolean("scheduleEnabled"),
                        schedule = (0 until windows.length()).map { index ->
                            val window = windows.getJSONObject(index)
                            val days = window.getJSONArray("days")
                            ScheduleWindow((0 until days.length()).map(days::getInt), window.getInt("startMinute"), window.getInt("endMinute"))
                        },
                        background = phone.getBoolean("background"), startOnOpen = phone.getBoolean("startOnOpen"),
                        startAfterBoot = phone.getBoolean("startAfterBoot"), preventSleep = phone.getBoolean("preventSleep"),
                        allowMetered = phone.getBoolean("allowMetered"), screenOffOnly = phone.getBoolean("screenOffOnly"),
                    ),
                    deviceId = id, controllerUrl = data.getString("controllerUrl"), installedVersion = installedVersion,
                    userStopped = changed || data.getBoolean("userStopped"),
                    prepareRequested = !changed && data.getBoolean("prepareRequested"),
                    workerInstalled = data.getBoolean("workerInstalled"),
                    drainingSince = if (data.isNull("drainingSince")) null else data.getLong("drainingSince"),
                    workerGeneration = data.getString("workerGeneration"),
                    drainDeadlineMillis = if (data.isNull("drainDeadlineMillis")) null else data.getLong("drainDeadlineMillis"),
                    resetRequest = data.optString("resetRequest").also { if (it.isNotEmpty()) UUID.fromString(it) },
                )
            } catch (error: Exception) {
                throw IllegalArgumentException("Saved settings could not be read; the original file has been preserved", error)
            }
        }
    }
}

fun PhonePolicy.sharedJson(): JSONObject = JSONObject().put("enabled", enabled)
    .put("resources", JSONObject().put("cpus", cpus).put("memoryMib", memoryMib).put("diskGib", diskGib))
    .put("allowBattery", allowBattery).put("minBatteryPercent", minBatteryPercent)
    .put("allowCi", allowCi).put("allowServices", allowServices).put("drainSeconds", drainSeconds)
    .put("idleOnly", false).put("idleAfterMinutes", 15).put("startAtLogin", false).put("background", background)
    .put("scheduleEnabled", scheduleEnabled).put("schedule", JSONArray(schedule.map {
        JSONObject().put("days", JSONArray(it.days)).put("startMinute", it.startMinute).put("endMinute", it.endMinute)
    }))

private fun PhonePolicy.phoneJson() = JSONObject().put("background", background).put("startOnOpen", startOnOpen)
    .put("startAfterBoot", startAfterBoot).put("preventSleep", preventSleep)
    .put("allowMetered", allowMetered).put("screenOffOnly", screenOffOnly)
