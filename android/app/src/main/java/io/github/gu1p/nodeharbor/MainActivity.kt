package io.github.gu1p.nodeharbor

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.core.net.toUri

class MainActivity : ComponentActivity() {
    private val agent get() = (application as NodeHarborApplication).agent
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { agent.refresh() }
    private val localNetworkPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { agent.refresh() }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val state by agent.state.collectAsState()
            NodeHarborApp(state, ::onAction) { url, code -> agent.enroll(url, code) }
        }
        if (intent.action == AppUpdates.REVIEW_INSTALLATION) agent.updates.reviewInstallation()
    }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); if (intent.action == AppUpdates.REVIEW_INSTALLATION) agent.updates.reviewInstallation() }
    override fun onStart() { super.onStart(); agent.visibility(true); agent.refresh(); agent.startAutomatically(StartCause.Open) }
    override fun onStop() { if (!isChangingConfigurations) agent.visibility(false); super.onStop() }
    private fun onAction(action: UiAction) {
        try {
            when (action) {
                UiAction.OpenBatterySettings -> startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                UiAction.OpenAppSettings -> startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, "package:$packageName".toUri()))
                UiAction.OpenUpdateSettings -> startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, "package:$packageName".toUri()))
                UiAction.RequestNotifications -> notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
                UiAction.RequestLocalNetwork -> if (android.os.Build.VERSION.SDK_INT >= 37) localNetworkPermission.launch(Manifest.permission.ACCESS_LOCAL_NETWORK)
                UiAction.CopyLogs -> getSystemService(ClipboardManager::class.java).setPrimaryClip(
                    ClipData.newPlainText("NodeHarbor activity", agent.state.value.activity.joinToString("\n")))
                else -> agent.action(action)
            }
        } catch (_: android.content.ActivityNotFoundException) { agent.showError("This phone does not provide that settings screen. Open NodeHarbor in Android Settings.") }
    }
}
