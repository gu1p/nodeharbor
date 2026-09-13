package io.github.gu1p.nodeharbor

import android.app.ActivityManager
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.StatFs
import org.json.JSONObject
import java.time.ZonedDateTime

data class PhoneSnapshot(val phone: PhoneObservation, val host: JSONObject, val shared: JSONObject, val permissions: PermissionState)

fun phoneSnapshot(context: Context, runtimeReady: Boolean, ownedDiskGib: Long = 0): PhoneSnapshot {
    val memory = ActivityManager.MemoryInfo()
    val manager = context.getSystemService(ActivityManager::class.java)
    manager.getMemoryInfo(memory)
    val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
    val plug = battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, -1) ?: -1
    val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
    val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
    val percent = if (level >= 0 && scale > 0) ((level.toLong() * 100 / scale).toInt()).coerceIn(0, 100) else null
    val power = context.getSystemService(PowerManager::class.java)
    val network = context.getSystemService(ConnectivityManager::class.java)
    val capabilities = network.getNetworkCapabilities(network.activeNetwork)
    val localNetworkAllowed = Build.VERSION.SDK_INT < 37 || context.checkSelfPermission(android.Manifest.permission.ACCESS_LOCAL_NETWORK) == android.content.pm.PackageManager.PERMISSION_GRANTED
    val freeDisk = StatFs(context.noBackupFilesDir.absolutePath).availableBytes / (1024L * 1024 * 1024)
    val host = JSONObject().put("cpus", Runtime.getRuntime().availableProcessors())
        .put("memoryMib", memory.totalMem / (1024 * 1024)).put("diskGib", freeDisk + ownedDiskGib.coerceAtLeast(0))
    val phone = PhoneObservation(Build.VERSION.SDK_INT, Build.SUPPORTED_ABIS.contains("arm64-v8a"),
        memory.availMem / (1024 * 1024), freeDisk, if (plug < 0) null else plug != 0, percent,
        network.isActiveNetworkMetered, capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true,
        runCatching { power.currentThermalStatus }.getOrNull(), power.isInteractive, runtimeReady, localNetworkAllowed)
    val now = ZonedDateTime.now()
    val shared = JSONObject().put("idleSeconds", JSONObject.NULL).put("onBattery", phone.charging?.not() ?: JSONObject.NULL)
        .put("batteryPercent", percent ?: JSONObject.NULL).put("weekday", now.dayOfWeek.value % 7)
        .put("minute", now.hour * 60 + now.minute).put("resources", host)
    return PhoneSnapshot(phone, host, shared, PermissionState(
        context.getSystemService(NotificationManager::class.java).areNotificationsEnabled(),
        power.isIgnoringBatteryOptimizations(context.packageName), manager.isBackgroundRestricted,
        Build.VERSION.SDK_INT >= 37, localNetworkAllowed))
}
