package io.github.gu1p.nodeharbor

import androidx.compose.foundation.layout.Row
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*

@Composable
internal fun AppUpdatesPanel(status: AppUpdateStatus, action: (UiAction) -> Unit) {
    val active = status.phase in setOf("checking", "downloading", "preparing", "waiting", "installing", "approval", "cancelling")
    HarborCard {
        Text("App updates", style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
        Text("Installed version: ${BuildConfig.VERSION_NAME}")
        status.availableVersion?.let { Text("Available version: $it") }
        Text(status.message, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        Row {
            Text("Automatic app updates", modifier = Modifier.weight(1f))
            Switch(checked = status.enabled, enabled = status.phase != "installing",
                onCheckedChange = { action(UiAction.Update(if (it) "enable" else "disable")) },
                modifier = Modifier.semantics { contentDescription = "Automatic app updates" })
        }
        Text("Updates wait for current jobs, preserve your sharing choices, and require Android installation approval.")
        if (!automaticReleaseUpdates(BuildConfig.VERSION_NAME, BuildConfig.DIRTY_SOURCE)) Text("Development builds support manual update checks. Automatic installation requires a signed release build.")
        if (status.phase == "downloading") Text("${status.downloaded / (1024 * 1024)} MiB downloaded" +
            (status.total?.let { " of ${it / (1024 * 1024)} MiB" } ?: ""))
        OutlinedButton(onClick = { action(UiAction.Update("check")) }, enabled = !active) { Text("Check for updates") }
        if (status.phase == "permission") Button(onClick = { action(UiAction.OpenUpdateSettings) }) { Text("Allow app installation") }
        else if (status.phase == "approval") Button(onClick = { action(UiAction.ReviewUpdate) }) { Text("Review Android installation") }
        else if (status.availableVersion != null) Button(onClick = { action(UiAction.Update("install")) }, enabled = !active) { Text("Install update") }
        if (active && status.phase != "installing") OutlinedButton(onClick = { action(UiAction.Update("cancel")) }) { Text("Cancel update") }
    }
}
