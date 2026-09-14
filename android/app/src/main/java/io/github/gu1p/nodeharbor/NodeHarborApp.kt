package io.github.gu1p.nodeharbor

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.error as semanticError
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp

data class FleetDevice(val name: String, val platform: String, val state: String, val reason: String)
data class UiState(
    val phoneName: String = "This phone",
    val phoneDescription: String = "Android · ARM64",
    val state: String = "paused",
    val reason: String = "Sharing is switched off",
    val policy: PhonePolicy = PhonePolicy(),
    val permissions: PermissionState = PermissionState(),
    val enrolled: Boolean = false,
    val workerInstalled: Boolean = false,
    val runtimeReason: String = "The packaged VM runtime is unavailable",
    val controllerUrl: String = "",
    val busy: Boolean = false,
    val error: String = "",
    val activity: List<String> = emptyList(),
    val fleet: List<FleetDevice> = emptyList(),
    val eligibleCi: Boolean = false,
    val eligibleServices: Boolean = false,
    val workloads: List<GuestPod>? = null,
    val updates: AppUpdateStatus = AppUpdateStatus(),
    val storage: StorageUiState = StorageUiState(),
)

sealed interface UiAction {
    data object ContinuousSetup : UiAction
    data object OpenBatterySettings : UiAction
    data object OpenAppSettings : UiAction
    data object RequestNotifications : UiAction
    data object RequestLocalNetwork : UiAction
    data object Prepare : UiAction
    data object Resume : UiAction
    data object Pause : UiAction
    data object Stop : UiAction
    data object Replace : UiAction
    data object RefreshFleet : UiAction
    data object CopyLogs : UiAction
    data object OpenUpdateSettings : UiAction
    data object ReviewUpdate : UiAction
    data class Update(val action: String) : UiAction
    data class Storage(val action: String) : UiAction
    data class ReviewStorage(val disks: List<PhoneStorageDisk>) : UiAction
    data class StorageRecovery(val enabled: Boolean) : UiAction
    data class SavePolicy(val policy: PhonePolicy) : UiAction
}

private val HarborColors = darkColorScheme(
    primary = Color(0xff75e1c6), onPrimary = Color(0xff102b25),
    background = Color(0xff10141d), onBackground = Color(0xffe9edf5),
    surface = Color(0xff19202d), onSurface = Color(0xffe9edf5),
    onSurfaceVariant = Color(0xff9ea9bb), outline = Color(0xff343b4b),
    error = Color(0xffffb3aa), secondaryContainer = Color(0xff233c3c),
)

@Composable
fun NodeHarborApp(state: UiState, onAction: (UiAction) -> Unit, onEnroll: (String, String) -> Unit) {
    var page by rememberSaveable { mutableStateOf("Your phone") }
    var confirmation by remember { mutableStateOf<UiAction?>(null) }
    var showSources by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val links = LocalUriHandler.current
    val credits = remember { context.assets.open("runtime-credits.txt").bufferedReader().use { it.readText() } }
    MaterialTheme(colorScheme = HarborColors) {
        Scaffold(
            containerColor = HarborColors.background,
            bottomBar = {
                NavigationBar(containerColor = Color(0xff151b26)) {
                    listOf("Your phone" to "◉", "Sharing rules" to "≡", "Fleet" to "▦", "Connection" to "↔").forEach { (title, glyph) ->
                        NavigationBarItem(selected = page == title, onClick = {
                            page = title
                            if (title == "Fleet") onAction(UiAction.RefreshFleet)
                        }, icon = { Text(glyph) }, label = { Text(title, maxLines = 2) },
                            modifier = Modifier.semantics { contentDescription = title })
                    }
                }
            },
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(18.dp)) {
                Text("⚓ nodeharbor", style = MaterialTheme.typography.headlineSmall, color = HarborColors.primary)
                Text(page, style = MaterialTheme.typography.headlineMedium, modifier = Modifier.semantics { heading() })
                if (state.error.isNotBlank()) HarborCard {
                    Text(state.error, color = HarborColors.error, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Assertive })
                }
                when (page) {
                    "Your phone" -> {
                        HarborCard {
                            Text(state.phoneName, style = MaterialTheme.typography.titleLarge)
                            Text(state.phoneDescription, color = HarborColors.onSurfaceVariant)
                            Text(state.reason, color = HarborColors.primary,
                                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                            if (state.runtimeReason.isNotBlank()) Text(state.runtimeReason)
                            if (!state.enrolled) Button(onClick = { page = "Connection" }) { Text("Connect to your fleet") }
                            else {
                                Button(onClick = { onAction(if (state.workerInstalled) UiAction.Resume else UiAction.Prepare) },
                                    enabled = !state.busy && state.runtimeReason.isBlank() && state.state !in listOf("stopping", "shutdown-unconfirmed")) {
                                    Text(if (state.workerInstalled) "Enable sharing" else "Prepare worker")
                                }
                                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    OutlinedButton(onClick = { onAction(UiAction.Pause) }) { Text("Pause") }
                                    OutlinedButton(onClick = { confirmation = UiAction.Stop }) { Text("Stop") }
                                }
                            }
                        }
                        HarborCard {
                            Text("Your contribution limits", style = MaterialTheme.typography.titleMedium)
                            Text("${state.policy.cpus} CPUs · ${state.policy.memoryMib / 1024} GiB RAM · ${state.policy.diskGib} GiB disk")
                            Text("Your phone. Your limits.", color = HarborColors.onSurfaceVariant)
                        }
                        WorkloadPanels(state.workloads)
                        AppUpdatesPanel(state.updates, onAction)
                        StoragePanel(state.storage, state.enrolled, onAction)
                        HarborCard {
                            Text("Worker activity", style = MaterialTheme.typography.titleMedium)
                            if (state.activity.isEmpty()) Text("Worker activity will appear here.", color = HarborColors.onSurfaceVariant)
                            else SelectionContainer { Text(state.activity.takeLast(100).joinToString("\n")) }
                            OutlinedButton(onClick = { onAction(UiAction.CopyLogs) }) { Text("Copy logs") }
                        }
                        if (state.workerInstalled) OutlinedButton(onClick = { confirmation = UiAction.Replace },
                            enabled = state.state !in listOf("stopping", "shutdown-unconfirmed")) { Text("Replace worker") }
                    }
                    "Sharing rules" -> SharingSettings(state, onAction)
                    "Connection" -> ConnectionSettings(state, onEnroll)
                    "Fleet" -> {
                        if (!state.enrolled) Text("Connect to your fleet to see its devices.")
                        else if (state.fleet.isEmpty()) Text("No fleet devices are available yet.")
                        state.fleet.forEach { device -> HarborCard {
                            Text(device.name, style = MaterialTheme.typography.titleMedium)
                            Text("${device.platform} · ${device.state}", color = HarborColors.primary)
                            Text(device.reason)
                        } }
                    }
                }
                Text("NodeHarbor ${BuildConfig.VERSION_NAME}", style = MaterialTheme.typography.labelSmall,
                    color = HarborColors.onSurfaceVariant)
                TextButton(onClick = { showSources = true }) { Text("Source and licenses") }
            }
        }
        if (showSources) AlertDialog(onDismissRequest = { showSources = false },
            title = { Text("Source and licenses") },
            text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("NodeHarbor ${BuildConfig.VERSION_NAME}")
                Text("Source commit: ${BuildConfig.SOURCE_COMMIT}")
                if (BuildConfig.DIRTY_SOURCE) Text("Development build with local source changes")
                Text(credits)
            } },
            confirmButton = { TextButton(onClick = {
                links.openUri("https://github.com/gu1p/nodeharbor/releases" +
                    if (BuildConfig.DIRTY_SOURCE) "" else "/tag/v${BuildConfig.VERSION_NAME}")
            }) { Text("Get corresponding sources") } },
            dismissButton = { TextButton(onClick = { showSources = false }) { Text("Close") } })
        confirmation?.let { action ->
            AlertDialog(onDismissRequest = { confirmation = null }, title = {
                Text(if (action == UiAction.Replace) "Replace this worker?" else "Stop this worker?")
            }, text = {
                Text(if (action == UiAction.Replace) "Running work drains, the worker’s access is removed, and its disk is deleted. Sharing stays off."
                    else "Stop accepting work and shut down the worker after draining. Sharing stays off until you enable it again.")
            }, confirmButton = { TextButton(onClick = { confirmation = null; onAction(action) }) {
                Text(if (action == UiAction.Replace) "Replace worker" else "Stop worker")
            } }, dismissButton = { TextButton(onClick = { confirmation = null }) { Text("Cancel") } })
        }
    }
}

@Composable
internal fun HarborCard(content: @Composable ColumnScope.() -> Unit) {
    Surface(shape = RoundedCornerShape(12.dp), color = HarborColors.surface, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
    }
}

@Composable
private fun SharingSettings(state: UiState, action: (UiAction) -> Unit) {
    var draft by remember(state.policy) { mutableStateOf(state.policy) }
    val invalidNumbers = remember { mutableStateMapOf<String, Boolean>() }
    val validateNumber: (String, Boolean) -> Unit = { label, valid -> invalidNumbers[label] = !valid }
    var schedules by remember(state.policy.schedule) { mutableStateOf(state.policy.schedule.map(ScheduleDraft::from)) }
    HarborCard {
        Text("Keep contributing", style = MaterialTheme.typography.titleMedium)
        Text("Continue while charging, including when you close the app or turn off the screen.")
        Button(onClick = { draft = draft.continuous(); action(UiAction.ContinuousSetup) }) { Text("Continuous sharing") }
        RuleSwitch("Start when opening the app", draft.startOnOpen) { draft = draft.copy(startOnOpen = it) }
        RuleSwitch("Continue after closing the app", draft.background) { draft = draft.copy(background = it) }
        RuleSwitch("Resume after restarting the phone", draft.startAfterBoot) { draft = draft.copy(startAfterBoot = it) }
        Text("After the first unlock. An explicit stop still takes priority.", style = MaterialTheme.typography.bodySmall)
        RuleSwitch("Keep processing with the screen off", draft.preventSleep) { draft = draft.copy(preventSleep = it) }
        RuleSwitch("Run while unplugged", draft.allowBattery) { draft = draft.copy(allowBattery = it) }
        NumberSetting("Minimum battery percent", draft.minBatteryPercent, validateNumber) { draft = draft.copy(minBatteryPercent = it) }
        RuleSwitch("Use metered connections", draft.allowMetered) { draft = draft.copy(allowMetered = it) }
        RuleSwitch("Run only with the screen off", draft.screenOffOnly) { draft = draft.copy(screenOffOnly = it) }
        Text("The worker pauses when your phone becomes too hot. Android can still stop the app.", style = MaterialTheme.typography.bodySmall)
    }
    HarborCard {
        Text("Phone setup", style = MaterialTheme.typography.titleMedium)
        Text("Battery optimization: ${if (state.permissions.batteryExempt) "exempt" else "not exempt"}")
        OutlinedButton(onClick = { action(UiAction.OpenBatterySettings) }) { Text("Battery settings") }
        Text("Notifications: ${if (state.permissions.notifications) "allowed" else "not allowed"}")
        OutlinedButton(onClick = { action(UiAction.RequestNotifications) }) { Text("Notification settings") }
        if (state.permissions.localNetworkRequired) {
            Text("Local network access: ${if (state.permissions.localNetworkAllowed) "allowed" else "not allowed"}")
            OutlinedButton(onClick = { action(UiAction.RequestLocalNetwork) }) { Text("Allow local network access") }
        }
        Text(if (state.permissions.backgroundRestricted) "Android restricts background activity" else "No Android background restriction detected")
        OutlinedButton(onClick = { action(UiAction.OpenAppSettings) }) { Text("App settings") }
        Text("Your phone may also have manufacturer settings for background activity and automatic start.", style = MaterialTheme.typography.bodySmall)
    }
    HarborCard {
        Text("Resources and workloads", style = MaterialTheme.typography.titleMedium)
        NumberSetting("CPU allowance", draft.cpus, validateNumber) { draft = draft.copy(cpus = it) }
        NumberSetting("Memory allowance in MiB", draft.memoryMib, validateNumber) { draft = draft.copy(memoryMib = it) }
        NumberSetting("Disk allowance in GiB", draft.diskGib, validateNumber) { draft = draft.copy(diskGib = it) }
        RuleSwitch("Accept CI jobs", draft.allowCi) { draft = draft.copy(allowCi = it) }
        RuleSwitch("Accept services", draft.allowServices) { draft = draft.copy(allowServices = it) }
        Text("Services require the fleet’s full reliability observation period.", style = MaterialTheme.typography.bodySmall)
        NumberSetting("Drain time in seconds", draft.drainSeconds, validateNumber) { draft = draft.copy(drainSeconds = it) }
    }
    HarborCard {
        Text("Sharing schedule", style = MaterialTheme.typography.titleMedium)
        RuleSwitch("Use a schedule", draft.scheduleEnabled) {
            draft = draft.copy(scheduleEnabled = it)
            if (it && schedules.isEmpty()) schedules = listOf(ScheduleDraft((0..6).toList(), "22:00", "06:00"))
        }
        Text("Times use this phone’s local time. Overnight windows finish the next day.", style = MaterialTheme.typography.bodySmall)
        if (draft.scheduleEnabled) schedules.forEachIndexed { index, schedule ->
            fun update(value: ScheduleDraft) { schedules = schedules.toMutableList().also { it[index] = value } }
            Text("Schedule ${index + 1}", style = MaterialTheme.typography.titleSmall)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                listOf("Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat").forEachIndexed { day, label ->
                    FilterChip(selected = day in schedule.days, onClick = {
                        update(schedule.copy(days = if (day in schedule.days) schedule.days - day else (schedule.days + day).sorted()))
                    }, label = { Text(label) }, modifier = Modifier.semantics { contentDescription = "Schedule ${index + 1} $label" })
                }
            }
            OutlinedTextField(value = schedule.start, onValueChange = { update(schedule.copy(start = it)) },
                label = { Text("Start time (HH:mm)") }, isError = clockMinute(schedule.start) == null,
                singleLine = true, modifier = Modifier.fillMaxWidth(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii))
            OutlinedTextField(value = schedule.end, onValueChange = { update(schedule.copy(end = it)) },
                label = { Text("End time (HH:mm)") }, isError = clockMinute(schedule.end) == null,
                singleLine = true, modifier = Modifier.fillMaxWidth(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii))
            if (schedule.window() == null) Text("Choose at least one day and different valid start and end times", color = HarborColors.error)
            TextButton(onClick = { schedules = schedules.filterIndexed { position, _ -> position != index } }) { Text("Remove schedule ${index + 1}") }
        }
        if (draft.scheduleEnabled) OutlinedButton(onClick = { schedules = schedules + ScheduleDraft((0..6).toList(), "22:00", "06:00") },
            enabled = schedules.size < 14) { Text("Add schedule") }
    }
    val validSchedule = !draft.scheduleEnabled || schedules.isNotEmpty() && schedules.all { it.window() != null }
    invalidNumbers.filterValues { it }.keys.forEach { label ->
        Text("$label: enter a whole number", color = HarborColors.error,
            style = MaterialTheme.typography.bodySmall, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Assertive })
    }
    Button(onClick = { action(UiAction.SavePolicy(draft.copy(schedule = schedules.mapNotNull { it.window() }))) },
        enabled = !state.busy && invalidNumbers.values.none { it } && validSchedule) { Text("Save sharing rules") }
}

@Composable
private fun RuleSwitch(label: String, value: Boolean, change: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, modifier = Modifier.weight(1f).padding(top = 12.dp))
        Switch(checked = value, onCheckedChange = change, modifier = Modifier.semantics { contentDescription = label })
    }
}

@Composable
private fun NumberSetting(label: String, value: Int, validate: (String, Boolean) -> Unit, change: (Int) -> Unit) {
    var text by remember(value) { mutableStateOf(value.toString()) }
    val valid = text.toIntOrNull() != null
    OutlinedTextField(value = text, onValueChange = { text = it; validate(label, it.toIntOrNull() != null); it.toIntOrNull()?.let(change) },
        label = { Text(label) }, singleLine = true, modifier = Modifier.fillMaxWidth().semantics {
            if (!valid) semanticError("$label: enter a whole number")
        },
        isError = !valid,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
}

@Composable
private fun ConnectionSettings(state: UiState, enroll: (String, String) -> Unit) {
    var url by remember(state.controllerUrl) { mutableStateOf(state.controllerUrl) }
    var code by remember { mutableStateOf("") }
    HarborCard {
        Text(if (state.enrolled) "Connected to your fleet" else "Connect to your fleet", style = MaterialTheme.typography.titleMedium)
        Text("Use the address and enrollment code supplied by your fleet administrator.")
        OutlinedTextField(value = url, onValueChange = { url = it }, label = { Text("Controller address") },
            singleLine = true, modifier = Modifier.fillMaxWidth(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
        OutlinedTextField(value = code, onValueChange = { code = it }, label = { Text("Enrollment code") },
            singleLine = true, visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth())
        Button(onClick = { enroll(url, code); code = "" }, enabled = !state.busy && !state.enrolled) { Text("Enroll this phone") }
        Text("Enrollment does not enable sharing.", style = MaterialTheme.typography.bodySmall)
    }
}
