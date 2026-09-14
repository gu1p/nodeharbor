package io.github.gu1p.nodeharbor

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.SystemClock
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/** Downloads and controller waits never share the owner-stop executor or UI mutex. */
class AppUpdates(private val context: Context, private val store: PrivateStore, private val supervisor: PhoneSupervisor,
                 private val publish: (AppUpdateStatus) -> Unit) {
    private val executor = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-updates").apply { isDaemon = true } }
    private val cleanup = Executors.newSingleThreadExecutor { Thread(it, "nodeharbor-update-cancel").apply { isDaemon = true } }
    private val running = AtomicBoolean(false)
    private val generation = AtomicLong()
    private val connection = AtomicReference<HttpURLConnection?>()
    private val mutation = Any()
    @Volatile private var status = AppUpdateStatus(enabled = store.load().automaticUpdates)
    @Volatile private var lastCheck = 0L
    @Volatile private var sessionId = -1
    @Volatile private var sessionOwner: Long? = null
    @Volatile private var approval: Intent? = null
    @Volatile private var permissionRequested = false
    @Volatile private var recoveringInstallation = false

    init {
        // An interrupted installer must not later replace a newly restarted VM.
        try {
            abandonInterruptedInstallations()
            if (store.load().applicationUpdatePending) supervisor.cancelApplicationUpdate()
        } catch (_: Exception) {
            recoveringInstallation = true
            report("cancelling", "Android could not cancel an interrupted installer. Retry Cancel update before restarting the worker.")
        }
        publish(status)
        executor.scheduleWithFixedDelay({
            if (automaticReleaseUpdates(BuildConfig.VERSION_NAME, BuildConfig.DIRTY_SOURCE) && store.load().automaticUpdates && (supervisor.visible || supervisor.supervised) &&
                (lastCheck == 0L || SystemClock.elapsedRealtime() - lastCheck >= TimeUnit.HOURS.toMillis(6))) enqueue(false)
        }, 30, 30, TimeUnit.SECONDS)
    }

    fun resumeAfterPermission() {
        if (permissionRequested && context.packageManager.canRequestPackageInstalls()) {
            permissionRequested = false
            enqueue(true)
        }
    }

    fun action(action: String) {
        if (status.phase == "installing") return
        when (action) {
            "enable", "disable" -> {
                store.update { it.copy(automaticUpdates = action == "enable") }
                report(status.phase, status.message)
                if (action == "disable") cancel() else enqueue(false)
            }
            "cancel" -> cancel()
            "check" -> enqueue(false)
            "install" -> enqueue(true)
        }
    }

    private fun report(phase: String, message: String, token: Long? = null, version: String? = status.availableVersion,
                       downloaded: Long = status.downloaded, total: Long? = status.total) = synchronized(mutation) {
        if (token != null && generation.get() != token) return@synchronized
        status = AppUpdateStatus(store.load().automaticUpdates, phase, message, version, downloaded, total)
        publish(status)
    }

    private fun cancel() {
        val previous: Long
        val abandoned: Int
        synchronized(mutation) {
            previous = generation.getAndIncrement()
            permissionRequested = false
            abandoned = sessionId
            if (abandoned < 0 && !recoveringInstallation) supervisor.cancelApplicationUpdate(previous)
            report("cancelling", "Cancelling the update. Your sharing choices are preserved.")
        }
        val activeConnection = connection.getAndSet(null)
        cleanup.execute {
            runCatching { activeConnection?.disconnect() }
            try {
                if (recoveringInstallation) abandonInterruptedInstallations()
                else if (abandoned >= 0) context.packageManager.packageInstaller.abandonSession(abandoned)
                supervisor.cancelApplicationUpdate(if (recoveringInstallation) null else previous)
                recoveringInstallation = false
            } catch (_: Exception) {
                if (generation.get() == previous + 1) report("cancelling", "Android could not cancel installation. Retry Cancel update; the worker remains stopped.")
                return@execute
            }
            if (sessionId == abandoned) { sessionId = -1; sessionOwner = null; approval = null }
            context.getSystemService(NotificationManager::class.java).cancel(UPDATE_NOTIFICATION)
            if (generation.get() == previous + 1) report("available", "Update cancelled. Your sharing rules still apply.")
        }
    }

    private fun enqueue(installRequested: Boolean) {
        if (status.phase in setOf("approval", "installing", "cancelling") || !running.compareAndSet(false, true)) return
        val token = generation.incrementAndGet()
        report("checking", "Checking for updates…", token, downloaded = 0, total = null)
        executor.execute {
            try { checkAndApply(token, installRequested) }
            catch (error: Exception) {
                if (generation.get() == token) report("error", when (error) {
                    is java.io.IOException -> "The update download is unavailable. Check your connection and retry."
                    else -> "Update could not finish: ${error.message ?: "verification or installation failed"}. Your sharing choices are preserved."
                }.take(2048), token)
            } finally { running.set(false) }
        }
    }

    private fun requireActive(token: Long) { check(generation.get() == token) { "The update was cancelled" } }

    private fun open(url: String, token: Long): HttpURLConnection {
        var current = requireUpdateHttps(url)
        repeat(6) {
            requireActive(token)
            val request = (current.toURL().openConnection() as HttpURLConnection).apply {
                connectTimeout = 30_000; readTimeout = 30_000; instanceFollowRedirects = false
                setRequestProperty("Cache-Control", "no-cache")
            }
            connection.set(request)
            try {
                requireActive(token)
                val code = request.responseCode
                if (code in setOf(301, 302, 303, 307, 308)) {
                    current = requireUpdateHttps(current.resolve(checkNotNull(request.getHeaderField("Location"))).toString())
                    request.disconnect(); connection.compareAndSet(request, null)
                } else {
                    check(code == 200) { "The update server did not provide the requested package" }
                    return request
                }
            } catch (error: Exception) { request.disconnect(); connection.compareAndSet(request, null); throw error }
        }
        error("The update server redirected too many times")
    }

    private fun checkAndApply(token: Long, installRequested: Boolean) {
        lastCheck = SystemClock.elapsedRealtime()
        val metadata = open(CHANNEL, token)
        val text = try { metadata.inputStream.use { input ->
            val bytes = input.readNBytes(1024 * 1024 + 1)
            check(bytes.size <= 1024 * 1024) { "The update response is too large" }
            bytes.toString(Charsets.UTF_8)
        } } finally { metadata.disconnect(); connection.compareAndSet(metadata, null) }
        requireActive(token)
        val candidate = parseAppUpdate(text, BuildConfig.VERSION_NAME)
        if (candidate == null) { report("idle", "No newer Android update is available", token, version = null); return }
        report("available", "NodeHarbor ${candidate.version} is available", token, version = candidate.version)
        if (!installRequested && (!store.load().automaticUpdates || !automaticReleaseUpdates(BuildConfig.VERSION_NAME, BuildConfig.DIRTY_SOURCE))) return
        if (!context.packageManager.canRequestPackageInstalls()) {
            permissionRequested = true
            report("permission", "Allow NodeHarbor to request app installation, then Android will ask you to approve this update.", token)
            return
        }
        val file = File.createTempFile("nodeharbor-update-", ".apk", context.cacheDir)
        try {
            applyAppUpdate(object : AppUpdateRuntime {
                override fun active() = generation.get() == token
                override fun downloadAndVerify() {
                    report("downloading", "Downloading and verifying the update…", token)
                    download(candidate, file, token)
                    verifyPackage(file, candidate)
                }
                override fun beginMaintenance() {
                    report("preparing", "Preparing the worker for an application update", token)
                    while (active()) {
                        val began = synchronized(mutation) { active() && supervisor.beginApplicationUpdate(token) }
                        if (began) return
                        Thread.sleep(1000)
                    }
                }
                override fun readiness(): UpdateGate {
                    val gate = supervisor.applicationUpdateGate(token)
                    report(if (gate == UpdateGate.Preparing) "preparing" else "waiting", when (gate) {
                        UpdateGate.Preparing -> "Preparing the worker for an application update"
                        UpdateGate.WaitingForController -> "Waiting for the fleet to confirm update maintenance"
                        UpdateGate.WaitingForJobs -> "Waiting for current jobs to finish. Updates do not evict jobs."
                        UpdateGate.Ready -> "Worker stopped. Ready for Android installation."
                        else -> "Waiting for Android to confirm worker shutdown"
                    }, token)
                    return gate
                }
                override fun awaitChange() { Thread.sleep(1000) }
                override fun install() {
                    synchronized(mutation) { requireActive(token); report("installing", "Handing the verified update to Android", token) }
                    verifyPackage(file, candidate)
                    installPackage(file, candidate, token)
                }
                override fun cancelMaintenance() { supervisor.cancelApplicationUpdate(token) }
            })
        } finally { file.delete() }
    }

    private fun download(candidate: AppUpdateCandidate, file: File, token: Long) {
        val request = open(candidate.url, token)
        val started = SystemClock.elapsedRealtime()
        try {
            val total = request.contentLengthLong.takeIf { it >= 0 }
            check(total == null || total <= MAX_BYTES) { "The update package exceeds its supported size" }
            request.inputStream.use { input -> FileOutputStream(file).use { output ->
                val buffer = ByteArray(65536); var count = 0L
                while (true) {
                    requireActive(token)
                    check(SystemClock.elapsedRealtime() - started < 600_000) { "The update download timed out" }
                    val size = input.read(buffer)
                    if (size < 0) break
                    count += size
                    check(count <= MAX_BYTES) { "The update package exceeds its supported size" }
                    output.write(buffer, 0, size)
                    report("downloading", "Downloading and verifying the update…", token, downloaded = count, total = total)
                }
                check(count > 0 && (total == null || count == total)) { "The update download is incomplete" }
                output.fd.sync()
            } }
        } finally { request.disconnect(); connection.compareAndSet(request, null) }
    }

    private fun verifyPackage(file: File, candidate: AppUpdateCandidate) {
        check(NativeRules.request(JSONObject().put("operation", "verifyUpdate").put("path", file.absolutePath)
            .put("signature", candidate.signature).toString()) == "true") { "The release signature does not authenticate this package" }
        val manager = context.packageManager
        val flags = PackageManager.PackageInfoFlags.of((PackageManager.GET_SIGNING_CERTIFICATES or PackageManager.GET_META_DATA).toLong())
        val current = manager.getPackageInfo(context.packageName, flags)
        val archive = checkNotNull(manager.getPackageArchiveInfo(file.absolutePath, flags)) { "The update is not an Android application" }
        check(archive.packageName == context.packageName && archive.longVersionCode > current.longVersionCode &&
            archive.longVersionCode == candidate.versionCode.toLong() && archive.versionName == candidate.version &&
            archive.applicationInfo?.metaData?.getString(SOURCE_META) == candidate.commit) { "The APK does not match its application, version and source identity" }
        val installedSigners = current.signingInfo?.apkContentsSigners?.map { it.toCharsString() }?.toSet()
        val updateSigners = archive.signingInfo?.apkContentsSigners?.map { it.toCharsString() }?.toSet()
        check(!installedSigners.isNullOrEmpty() && installedSigners == updateSigners) { "The update does not match the installed Android signing identity" }
    }

    private fun installPackage(file: File, candidate: AppUpdateCandidate, token: Long) {
        check(supervisor.applicationUpdateGate(token) == UpdateGate.Ready) { "Wait for confirmed worker shutdown" }
        val installer = context.packageManager.packageInstaller
        val parameters = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(file.length())
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
            setPackageSource(PackageInstaller.PACKAGE_SOURCE_DOWNLOADED_FILE)
        }
        val id = installer.createSession(parameters)
        sessionId = id; sessionOwner = token
        try {
            installer.openSession(id).use { session ->
                session.openWrite("base.apk", 0, file.length()).use { output ->
                    file.inputStream().use { it.copyTo(output, 65536) }
                    session.fsync(output)
                }
                store.update { it.copy(updateInstallVersion = candidate.versionCode) }
                val result = PendingIntent.getBroadcast(context, id, Intent(context, UpdateInstallReceiver::class.java),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE)
                session.commit(result.intentSender)
            }
        } catch (error: Exception) { installer.abandonSession(id); sessionId = -1; sessionOwner = null; throw error }
    }

    fun installerResult(intent: Intent) {
        if (intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1) != sessionId) return
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                approval = intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                if (approval == null) { cancel(); return }
                report("approval", "Android needs your approval to install the verified update")
                if (supervisor.visible) reviewInstallation() else notifyApproval()
            }
            PackageInstaller.STATUS_SUCCESS -> {
                supervisor.cancelApplicationUpdate(sessionOwner)
                sessionId = -1; sessionOwner = null; approval = null
                report("idle", "The Android update was installed")
            }
            else -> {
                supervisor.cancelApplicationUpdate(sessionOwner)
                sessionId = -1; sessionOwner = null; approval = null
                report("available", "Android did not install the update. You can retry; your sharing choices are preserved.")
            }
        }
    }

    fun reviewInstallation() {
        val intent = approval ?: return
        context.startActivity(Intent(intent).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    private fun abandonInterruptedInstallations() {
        val installer = context.packageManager.packageInstaller
        for (session in installer.mySessions.filter { it.appPackageName == context.packageName })
            installer.abandonSession(session.sessionId)
    }

    private fun notifyApproval() {
        val manager = context.getSystemService(NotificationManager::class.java)
        val channel = "nodeharbor-updates"
        manager.createNotificationChannel(NotificationChannel(channel, "App updates", NotificationManager.IMPORTANCE_DEFAULT))
        val review = PendingIntent.getActivity(context, UPDATE_NOTIFICATION,
            Intent(context, MainActivity::class.java).setAction(REVIEW_INSTALLATION), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(context, channel).setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle("NodeHarbor update ready").setContentText("Open NodeHarbor to review Android installation")
            .setContentIntent(review).setAutoCancel(true).build()
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED)
            manager.notify(UPDATE_NOTIFICATION, notification)
    }

    companion object {
        const val REVIEW_INSTALLATION = "io.github.gu1p.nodeharbor.REVIEW_UPDATE"
        private const val SOURCE_META = "io.github.gu1p.nodeharbor.SOURCE_COMMIT"
        private const val CHANNEL = "https://gu1p.github.io/nodeharbor/updates/latest.json"
        private const val MAX_BYTES = 512L * 1024 * 1024
        private const val UPDATE_NOTIFICATION = 2
    }
}

class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        (context.applicationContext as NodeHarborApplication).agent.updates.installerResult(intent)
    }
}
