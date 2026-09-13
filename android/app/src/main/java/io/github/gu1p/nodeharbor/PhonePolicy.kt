package io.github.gu1p.nodeharbor

data class ScheduleWindow(val days: List<Int>, val startMinute: Int, val endMinute: Int)

data class PhonePolicy(
    val enabled: Boolean = false,
    val cpus: Int = 2,
    val memoryMib: Int = 2048,
    val diskGib: Int = 15,
    val allowBattery: Boolean = false,
    val minBatteryPercent: Int = 30,
    val allowCi: Boolean = true,
    val allowServices: Boolean = false,
    val drainSeconds: Int = 300,
    val scheduleEnabled: Boolean = false,
    val schedule: List<ScheduleWindow> = emptyList(),
    val background: Boolean = false,
    val startOnOpen: Boolean = false,
    val startAfterBoot: Boolean = false,
    val preventSleep: Boolean = false,
    val allowMetered: Boolean = false,
    val screenOffOnly: Boolean = false,
) {
    fun continuous() = copy(background = true, startOnOpen = true, startAfterBoot = true, preventSleep = true)
}

data class PhoneObservation(
    val sdk: Int,
    val arm64: Boolean,
    val availableMemoryMib: Long,
    val freeDiskGib: Long,
    val charging: Boolean?,
    val batteryPercent: Int?,
    val metered: Boolean,
    val networkAvailable: Boolean,
    val thermal: Int?,
    val screenOn: Boolean,
    val runtimeReady: Boolean,
    val localNetworkAllowed: Boolean = true,
)

data class PermissionState(
    val notifications: Boolean = false,
    val batteryExempt: Boolean = false,
    val backgroundRestricted: Boolean = false,
    val localNetworkRequired: Boolean = false,
    val localNetworkAllowed: Boolean = false,
)

data class PhoneDecision(val allowed: Boolean, val reason: String)

/** Additional phone constraints; the shared Rust owner policy must also allow work. */
fun phoneDecision(policy: PhonePolicy, phone: PhoneObservation, requestedMemoryMib: Int): PhoneDecision {
    val reason = when {
        phone.sdk < 33 || !phone.arm64 -> "This worker requires an ARM64 phone with Android 13 or newer"
        !phone.runtimeReady -> "The packaged VM runtime is unavailable"
        phone.sdk >= 37 && !phone.localNetworkAllowed -> "Allow local network access in Sharing rules before starting the worker"
        phone.charging == null -> "Power information is unavailable"
        !phone.charging && !policy.allowBattery -> "Paused while running on battery"
        !phone.charging && (phone.batteryPercent == null || phone.batteryPercent < policy.minBatteryPercent) ->
            "Battery is below your chosen minimum or unavailable"
        phone.thermal == null -> "Phone temperature information is unavailable"
        phone.thermal >= 3 -> "Paused to let your phone cool down"
        !phone.networkAvailable -> "Waiting for a network connection"
        phone.metered && !policy.allowMetered -> "Waiting for an unmetered connection"
        phone.availableMemoryMib < requestedMemoryMib.coerceAtLeast(0).toLong() + 512 ->
            "Waiting for enough available memory for the worker"
        policy.screenOffOnly && phone.screenOn -> "Waiting for the screen to turn off"
        else -> null
    }
    return PhoneDecision(reason == null, reason ?: "Your phone sharing rules allow work")
}

enum class StartCause { Open, Boot, Recovery }

fun shouldAutoStart(policy: PhonePolicy, cause: StartCause, sharingEnabled: Boolean,
                    userStopped: Boolean, appUpdated: Boolean): Boolean {
    if (!sharingEnabled || userStopped || appUpdated) return false
    return when (cause) {
        StartCause.Open -> policy.startOnOpen
        StartCause.Boot -> policy.startAfterBoot && policy.background
        StartCause.Recovery -> policy.background
    }
}
