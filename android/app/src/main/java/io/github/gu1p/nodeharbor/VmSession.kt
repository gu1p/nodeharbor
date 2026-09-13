package io.github.gu1p.nodeharbor

import android.content.Context
import android.os.ParcelFileDescriptor
import android.os.Process
import android.os.SystemClock
import android.system.Os
import android.system.OsConstants
import org.json.JSONObject
import java.io.Closeable
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

data class GuestPod(val uid: String, val namespace: String)
data class GuestStatus(val configured: Boolean, val preparing: Boolean, val running: Boolean, val pods: List<GuestPod>?, val error: String,
                       val controlRevision: String = GUEST_CONTROL_REVISION) {
    fun workloadCount(systemPodUids: Set<String>): Int = pods?.count { it.namespace != "kube-system" && it.uid !in systemPodUids } ?: Int.MAX_VALUE
}

/** The main process supervises one owned VM and revokes its capabilities on stop. */
class VmSession private constructor(private val context: Context, private val owner: String,
                                    val policy: PhonePolicy, private val networkAllowed: () -> Boolean,
                                    private val record: (String) -> Unit,
                                    private val privateConsole: ((ByteArray) -> Unit)? = null) : Closeable {
    private val stopping = AtomicBoolean(false)
    private val dead = CountDownLatch(1)
    private val coordinator = ShutdownCoordinator()
    private val completion = CompletableFuture<ShutdownOutcome>()
    private val cleaned = AtomicBoolean(false)
    @Volatile private var storageReservation: Closeable? = null
    private val revoked = AtomicBoolean(false)
    private val monitor = Executors.newSingleThreadScheduledExecutor { Thread(it, "nodeharbor-vm-deadline").apply { isDaemon = true } }
    private val resourceLock = Any()
    @Volatile private var observer: (ShutdownOutcome) -> Unit = {}
    private var reported: ShutdownOutcome? = null
    @Volatile private var launchFinished = false
    @Volatile private var bootSubmitted = false
    val shutdownOutcome: ShutdownOutcome? get() = coordinator.outcome
    val isStopping: Boolean get() = stopping.get()
    fun observeShutdown(listener: (ShutdownOutcome) -> Unit) { observer = listener; shutdownOutcome?.let(listener) }
    init { monitor.scheduleWithFixedDelay({ handle(coordinator.poll(SystemClock.elapsedRealtime())) }, 0, 50, TimeUnit.MILLISECONDS) }
    private val threads = Executors.newCachedThreadPool { task -> Thread(task, "nodeharbor-vm-owner").apply { isDaemon = true } }
    private val broker = NetworkBroker(context) { !stopping.get() && networkAllowed() }
    private var binding: SandboxBinding? = null
    private var channel: GuestChannel? = null
    private var console: ParcelFileDescriptor? = null
    @Volatile private var launched = false
    val running: Boolean get() = (launched || !launchFinished) && dead.count != 0L
    val confirmedStopped: Boolean get() = dead.count == 0L

    private fun launch(guest: OwnedGuest) {
        val connection = SandboxBinding(context, disconnected = { record("The isolated worker binding was lost; waiting for process death") },
            processDied = { terminated() }, autoConnect = false)
        synchronized(resourceLock) { check(!stopping.get()); binding = connection }
        connection.connect()
        check(!stopping.get()) { "The owner stopped VM startup" }
        val sandbox = connection.service
        val inspection = sandbox.inspect(guest.file("owner.json").absolutePath)
        val expected = context.assets.open("lock.json").bufferedReader().use { JSONObject(it.readText()) }
            .getJSONObject("qemu").getString("version")
        check(inspection.getInt("uid") != Process.myUid() && !inspection.getBoolean("canReadPrivateFile") &&
            !inspection.getBoolean("canOpenNetwork") && inspection.getString("runtimeVersion") == expected) {
            "The packaged VM runtime did not pass its isolation check"
        }
        val output = ParcelFileDescriptor.createSocketPair()
        val control = ParcelFileDescriptor.createSocketPair()
        val files = mutableListOf<ParcelFileDescriptor>()
        try {
            synchronized(resourceLock) {
                check(!stopping.get() && !confirmedStopped) { "The owner stopped VM startup" }
                console = output[0]
                channel = GuestChannel(control[0], owner)
            }
            threads.execute { consumeConsole(output[0]) }
            for ((index, name) in listOf("root.img", "kernel", "initrd", "seed.iso", "root.img").withIndex()) {
                files += ParcelFileDescriptor.open(guest.file(name), if (index == 0) ParcelFileDescriptor.MODE_READ_WRITE else ParcelFileDescriptor.MODE_READ_ONLY)
            }
            check(!stopping.get()) { "The owner stopped VM startup" }
            bootSubmitted = true
            sandbox.boot(files[0], files[1], files[2], files[3], output[1], control[1], files[4], broker, policy.cpus, policy.memoryMib)
            launched = true
            check(!confirmedStopped) { "The isolated worker stopped during startup" }
            record("Starting the owned Ubuntu worker")
        } finally {
            files.forEach { runCatching { it.close() } }
            output[1].close(); control[1].close()
            if (!launched) (output + control).forEach { runCatching { it.close() } }
        }
    }
    private fun consumeConsole(file: ParcelFileDescriptor) {
        // Drain output continuously without retaining raw guest logs or secrets.
        // Recognized boot stages become bounded, fixed owner-visible messages.
        val stages = mutableSetOf<String>()
        var diagnosticBytes = 0
        val line = StringBuilder()
        try {
            ParcelFileDescriptor.AutoCloseInputStream(file).use { input ->
                val bytes = ByteArray(8192)
                while (true) {
                    val count = input.read(bytes)
                    if (count < 0) break
                    if (privateConsole != null && diagnosticBytes + count <= 4 * 1024 * 1024) {
                        privateConsole.invoke(bytes.copyOf(count)); diagnosticBytes += count
                    }
                    for (index in 0 until count) {
                        val char = (bytes[index].toInt() and 255).toChar()
                        if (char == '\n' || char == '\r') {
                            val text = line.toString()
                            if (Regex("\\[ *[0-9.]+] reboot: Power down").matches(text.trim())) {
                                coordinator.guestPoweredOff()
                                record("Guest poweroff observed")
                            }
                            val stage = when {
                                text.contains("NODEHARBOR_CONTROL poweroff-received") -> "Guest received the shutdown command"
                                text.contains("NODEHARBOR_CONTROL shutdown-initiated") -> "Guest initiated system shutdown"
                                text.contains("Run /init") -> "Ubuntu is starting its system services"
                                text.contains("cloud-init-local.service") -> "Ubuntu is applying the owned worker configuration"
                                text.contains("nodeharbor-control.service") -> "Ubuntu is starting its private owner channel"
                                else -> null
                            }
                            if (stage != null && stages.add(stage)) record(stage)
                            line.clear()
                        } else if (char.code in 32..126 && line.length < 2048) line.append(char)
                    }
                }
            }
        } catch (_: Exception) { /* Only Binder death confirms process termination. */ }
        finally { handle(coordinator.consoleClosed(SystemClock.elapsedRealtime())) }
    }
    fun status(timeoutMillis: Long = 15_000): GuestStatus {
        check(!stopping.get()) { "The worker is stopping" }
        val value = checkNotNull(channel).request(GuestCommand.Status, timeoutMillis = timeoutMillis)
        require(value.getString("architecture") == "aarch64") { "The owned guest reported an unsupported CPU architecture" }
        val items = if (value.isNull("pods")) null else value.getJSONArray("pods")
        require(items == null || items.length() <= 4096) { "The guest workload inventory exceeds its supported size" }
        val pods = items?.let { list -> (0 until list.length()).map { index ->
            val item = list.getJSONObject(index)
            val uid = item.getString("uid")
            val namespace = item.getString("namespace")
            require(uid.length in 1..128 && namespace.length in 1..253) { "The guest returned an invalid workload inventory" }
            GuestPod(uid, namespace)
        } }
        return GuestStatus(value.getBoolean("configured"), value.getBoolean("preparing"), value.getBoolean("running"), pods,
            if (value.optString("error").isBlank()) "" else "The guest could not prepare the worker; check fleet connectivity and retry", value.optString("controlRevision"))
    }
    fun configure(bootstrap: JSONObject) { check(!stopping.get()); checkNotNull(channel).request(GuestCommand.Configure, bootstrap) }
    fun renewLease() { check(!stopping.get()); checkNotNull(channel).request(GuestCommand.Lease) }
    private fun requestPoweroff() {
        record("Owner submitted the shutdown command")
        checkNotNull(channel).request(GuestCommand.Poweroff, timeoutMillis = 3000)
        record("Guest acknowledged the shutdown command")
    }

    private fun terminated() {
        storageReservation?.close()
        dead.countDown()
        record("Android confirmed isolated process death")
        revoke()
        binding?.close()
        handle(coordinator.processDied(SystemClock.elapsedRealtime()))
    }
    private fun revoke() {
        synchronized(resourceLock) {
            if (revoked.compareAndSet(false, true)) { broker.close(); channel?.close() }
        }
    }
    /** Does not wait for any in-flight controller, startup or guest command. */
    fun shutdown(drainDeadline: Long, immediate: Boolean = false): CompletableFuture<ShutdownOutcome> {
        stopping.set(true)
        handle(coordinator.stop(SystemClock.elapsedRealtime(), drainDeadline, immediate))
        return completion
    }
    private fun handle(update: ShutdownUpdate) {
        if (update.poweroff) threads.execute {
            try { requestPoweroff() }
            catch (_: Exception) {
                record("The shutdown command could not complete")
                shutdown(SystemClock.elapsedRealtime(), immediate = true)
            }
        }
        if (update.force) {
            record("Forcing isolated worker teardown")
            revoke()
            // The original Binder remains observable after unbinding. Never use
            // a connection callback or a stop acknowledgement as death evidence.
            val remote = binding?.original
            if (remote != null) threads.execute { runCatching { remote.stop() } }
            binding?.close()
            if (launchFinished && !bootSubmitted && remote == null) terminated()
        }
        val outcome = update.outcome ?: return
        synchronized(this) {
            if (reported == outcome || (reported != null && reported != ShutdownOutcome.Unconfirmed)) return
            reported = outcome
        }
        if (cleaned.compareAndSet(false, true)) {
            revoke(); binding?.close()
            console?.let { runCatching { Os.shutdown(it.fileDescriptor, OsConstants.SHUT_RDWR) }; runCatching { it.close() } }
            monitor.shutdownNow(); threads.shutdownNow()
        }
        observer(outcome)
        completion.complete(outcome)
    }
    /** Compatibility for callers requiring bounded forced termination. */
    fun halt(timeoutMillis: Long = 5000): Boolean {
        shutdown(SystemClock.elapsedRealtime(), immediate = true)
        return dead.await(timeoutMillis, TimeUnit.MILLISECONDS)
    }
    override fun close() { halt() }

    companion object {
        fun start(context: Context, owner: String, policy: PhonePolicy, guest: OwnedGuest,
                  networkAllowed: () -> Boolean, record: (String) -> Unit,
                  privateConsole: ((ByteArray) -> Unit)? = null,
                  created: (VmSession) -> Unit = {}): VmSession {
            check(networkAllowed()) { "The owner has stopped worker processing" }
            val receipt = checkNotNull(guest.receipt(owner)) { "Prepare an owned worker first" }
            check(receipt.complete && receipt.diskGib == policy.diskGib) { "The owned worker storage is incomplete or needs replacement" }
            check(guest.file("root.img").length() == receipt.diskGib * 1024L * 1024 * 1024) { "The owned worker disk changed size" }
            for (name in listOf("kernel", "initrd", "seed.iso")) check(guest.file(name).isFile && guest.file(name).length() in 1..(128L * 1024 * 1024)) {
                "The owned worker boot files are missing or invalid"
            }
            val reservation = guest.reserveForVm(owner)
            val session = VmSession(context, owner, policy, networkAllowed, record, privateConsole)
            session.storageReservation = reservation
            try { created(session); session.launch(guest); return session }
            catch (error: Exception) { session.shutdown(SystemClock.elapsedRealtime(), immediate = true); throw error }
            finally {
                session.launchFinished = true
                if (!session.bootSubmitted && session.binding?.original == null) session.terminated()
            }
        }
    }
}
