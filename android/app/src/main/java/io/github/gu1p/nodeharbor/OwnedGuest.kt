package io.github.gu1p.nodeharbor

import android.content.Context
import android.system.Os
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.Closeable
import java.io.File
import java.io.RandomAccessFile
import java.net.URI
import java.nio.file.Files
import java.security.MessageDigest
import java.util.UUID
import java.util.zip.GZIPInputStream
import javax.net.ssl.HttpsURLConnection

/** This directory and receipt are owned by the app; the emulator only gets FDs. */
class OwnedGuest(private val context: Context, val directory: File = context.noBackupFilesDir.resolve("nodeharbor/worker")) {
    private val names = setOf("owner.json", "owner.json.bak", "owner.json.new", "root.img", "root.img.preparing", "seed.iso", "seed.iso.bak", "seed.iso.new",
        "disk.tar.gz", "disk.tar.gz.partial", "kernel", "kernel.partial", "initrd", "initrd.partial")
    fun file(name: String): File {
        require(name in names)
        val file = directory.resolve(name)
        check(!Files.isSymbolicLink(directory.toPath()) && !Files.isSymbolicLink(file.toPath())) { "Unexpected worker storage link; files have been preserved" }
        return file
    }
    @Synchronized fun receipt(owner: String): GuestReceipt? {
        val stored = AtomicFile(file("owner.json"))
        if (!stored.baseFile.exists() && !file("owner.json.bak").exists()) {
            check(!directory.exists() || directory.listFiles()?.isEmpty() == true) { "Unrecognized worker files have been preserved" }
            return null
        }
        return GuestReceipt.decode(stored.openRead().use { it.readNBytes(16385).toString(Charsets.UTF_8) }, owner)
    }
    @Synchronized fun claim(owner: String, diskGib: Int, imageVersion: String): GuestReceipt {
        require(UUID.fromString(owner).toString() == owner && diskGib in 15..65536)
        val existing = receipt(owner)
        if (existing != null) {
            check(existing.diskGib == diskGib && existing.imageVersion == imageVersion) { "Replace the worker to change its disk allocation or guest image" }
            return existing
        }
        check(directory.isDirectory || directory.mkdirs()) { "Private worker storage is unavailable" }
        return GuestReceipt(owner, UUID.randomUUID().toString(), diskGib, imageVersion, false).also(::saveReceipt)
    }
    @Synchronized private fun saveReceipt(value: GuestReceipt) {
        val storage = AtomicFile(file("owner.json"))
        val output = storage.startWrite()
        try { output.write(value.encode().toByteArray()); storage.finishWrite(output) }
        catch (error: Exception) { storage.failWrite(output); throw error }
    }
    @Synchronized fun allocatedBytes(owner: String): Long {
        if (owner.isEmpty() || receipt(owner) == null) return 0
        return listOf("root.img", "root.img.preparing").sumOf { name ->
            val image = file(name)
            try { Os.stat(image.absolutePath).st_blocks * 512 }
            catch (error: android.system.ErrnoException) {
                if (error.errno == android.system.OsConstants.ENOENT) 0L else throw error
            }
        }
    }
    fun remove(owner: String, stopped: Boolean, reset: Boolean) = reserve().use {
        removeStopped(owner, stopped, reset)
    }
    @Synchronized private fun removeStopped(owner: String, stopped: Boolean, reset: Boolean) {
        val owned = checkNotNull(receipt(owner)) { "No owned worker receipt was found" }
        check(canRemoveGuest(owned, owner, stopped, reset)) { "Confirm worker shutdown and fleet access removal before deleting its disk" }
        val entries = checkNotNull(directory.listFiles()) { "Worker storage is unavailable" }
        check(entries.all { it.name in names && it.isFile && !Files.isSymbolicLink(it.toPath()) }) { "Unrecognized worker files have been preserved" }
        // Keep the receipt until the other owned files have actually gone.
        for (entry in entries.sortedBy { it.name.startsWith("owner.json") }) check(entry.delete()) { "An owned worker file could not be removed" }
        check(directory.delete()) { "The worker directory could not be removed" }
    }
    fun prepare(owner: String, policy: PhonePolicy, active: () -> Boolean, progress: (String) -> Unit): GuestReceipt =
        reserve().use { prepareStopped(owner, policy, active, progress) }
    private fun prepareStopped(owner: String, policy: PhonePolicy, active: () -> Boolean, progress: (String) -> Unit): GuestReceipt {
        val specification = context.assets.open("lock.json").bufferedReader().use { JSONObject(it.readText()) }.getJSONObject("guest")
        val owned = claim(owner, policy.diskGib, specification.getString("version"))
        check(active()) { "Worker preparation was stopped by your sharing rules" }
        if (!owned.complete) {
            val disk = specification.getJSONObject("disk")
            download("disk.tar.gz", disk, active, progress)
            progress("Verifying and unpacking the Ubuntu worker disk")
            val staging = file("root.img.preparing")
            try {
                GZIPInputStream(file("disk.tar.gz").inputStream().buffered()).use { input ->
                    staging.outputStream().buffered().use { output ->
                        extractGuestImage(input, output, disk.getString("entry"), disk.getLong("imageBytes"), disk.getString("imageSha256"), active)
                    }
                }
                check(active()) { "Worker preparation was stopped by your sharing rules" }
                progress("Reserving ${policy.diskGib} GiB for your worker")
                RandomAccessFile(staging, "rw").use { image ->
                    val size = policy.diskGib * 1024L * 1024 * 1024
                    image.setLength(size)
                    Os.posix_fallocate(image.fd, 0, size)
                    image.fd.sync()
                }
                check(active()) { "Worker preparation was stopped by your sharing rules" }
                Files.move(staging.toPath(), file("root.img").toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING)
            } finally { staging.delete() }
        }
        for (name in listOf("kernel", "initrd")) download(name, specification.getJSONObject(name), active, progress)
        check(file("root.img").isFile && file("root.img").length() == policy.diskGib * 1024L * 1024 * 1024) { "The owned worker disk is missing or has changed size" }
        writeSeed(owned)
        val complete = owned.copy(complete = true)
        saveReceipt(complete)
        file("disk.tar.gz").delete()
        return complete
    }
    fun refreshSeed(owned: GuestReceipt, stopped: Boolean) {
        check(stopped) { "Wait for Android to confirm worker shutdown before refreshing its boot seed" }
        reserve().use { writeSeed(owned) }
    }
    private fun writeSeed(owned: GuestReceipt) {
        check(receipt(owned.deviceId) == owned) { "The owned worker receipt changed" }
        val storage = AtomicFile(file("seed.iso"))
        val output = storage.startWrite()
        try { output.write(workerSeed(owned)); storage.finishWrite(output) }
        catch (error: Exception) { storage.failWrite(output); throw error }
    }
    /** Held through unconfirmed teardown; another OwnedGuest object cannot bypass it. */
    fun reserveForVm(owner: String): Closeable {
        val reservation = reserve()
        try { writeSeed(checkNotNull(receipt(owner))); return reservation }
        catch (error: Exception) { reservation.close(); throw error }
    }
    private fun reserve(): Closeable {
        val path = directory.canonicalPath
        val token = Any()
        check(reservations.putIfAbsent(path, token) == null) { "Wait for Android to confirm worker shutdown; its files have been preserved" }
        return Closeable { reservations.remove(path, token) }
    }
    companion object { private val reservations = java.util.concurrent.ConcurrentHashMap<String, Any>() }
    private fun sha(file: File): String = file.inputStream().use { input ->
        val hash = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(1024 * 1024)
        while (true) { val count = input.read(buffer); if (count < 0) break; hash.update(buffer, 0, count) }
        hash.digest().joinToString("") { "%02x".format(it) }
    }
    private fun download(name: String, specification: JSONObject, active: () -> Boolean, progress: (String) -> Unit) {
        val target = file(name)
        val expected = specification.getString("sha256")
        val maximum = specification.getLong("maximumBytes")
        if (target.isFile && target.length() <= maximum && sha(target) == expected) return
        val url = URI(specification.getString("url"))
        require(url.scheme == "https" && url.host == "cloud-images.ubuntu.com" && url.userInfo == null)
        val connection = url.toURL().openConnection() as HttpsURLConnection
        val partial = file("$name.partial")
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 30_000; connection.readTimeout = 30_000
            check(connection.responseCode == 200) { "The verified Ubuntu download is unavailable; retry preparation later" }
            require(connection.contentLengthLong <= maximum) { "The Ubuntu download exceeds its supported size" }
            progress("Downloading and verifying Ubuntu $name")
            val hash = MessageDigest.getInstance("SHA-256")
            connection.inputStream.use { input -> partial.outputStream().use { output ->
                val buffer = ByteArray(1024 * 1024)
                var total = 0L
                var reported = 0L
                while (true) {
                    check(active()) { "Worker preparation was stopped by your sharing rules" }
                    val count = input.read(buffer)
                    if (count < 0) break
                    total += count
                    require(total <= maximum) { "The Ubuntu download exceeds its supported size" }
                    hash.update(buffer, 0, count); output.write(buffer, 0, count)
                    if (total - reported >= 32L * 1024 * 1024) { reported = total; progress("Downloading Ubuntu $name: ${total / (1024 * 1024)} MiB") }
                }
                require(hash.digest().joinToString("") { "%02x".format(it) } == expected) { "The Ubuntu download failed SHA-256 verification" }
                output.fd.sync()
            } }
            Files.move(partial.toPath(), target.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING)
        } finally { connection.disconnect(); partial.delete() }
    }
    internal fun workerSeed(receipt: GuestReceipt): ByteArray {
        val writes = JSONArray()
        fun write(path: String, content: String, mode: String = "0700") {
            writes.put(JSONObject().put("path", path).put("owner", "root:root").put("permissions", mode).put("content", content))
        }
        write("/etc/nodeharbor/device-id", receipt.deviceId + "\n", "0600")
        for (name in listOf("configure_worker.py", "watchdog.py", "android_control.py", "storage_pool.py", "storage_backup.py", "android_storage.py"))
            write("/usr/local/lib/nodeharbor/$name", context.assets.open(name).bufferedReader().use { it.readText() })
        write("/etc/systemd/system/nodeharbor-watchdog.service", "[Unit]\nDescription=Check the NodeHarbor owner lease\n[Service]\nType=oneshot\nExecStart=/usr/bin/python3 /usr/local/lib/nodeharbor/watchdog.py\n", "0644")
        // Lease monitoring waits for the channel. Do not implicitly put this
        // timer before timers.target/basic.target: its channel needs services
        // that start after basic.target. Keep shutdown ordering explicit.
        write("/etc/systemd/system/nodeharbor-watchdog.timer", "[Unit]\nDescription=Check the NodeHarbor owner lease regularly\nDefaultDependencies=no\nRequires=sysinit.target\nAfter=sysinit.target\nWants=nodeharbor-control.service\nAfter=nodeharbor-control.service\nBefore=shutdown.target\nConflicts=shutdown.target\n[Timer]\nOnActiveSec=15s\nOnUnitActiveSec=15s\nAccuracySec=1s\n[Install]\nWantedBy=multi-user.target\n", "0644")
        // Normal systemctl poweroff uses D-Bus and logind. Accepting owner
        // commands before they start caused the repeated-shutdown timeout.
        // Android's independent deadline still handles cancellation during boot.
        write("/etc/systemd/system/nodeharbor-control.service", "[Unit]\nDescription=NodeHarbor private owner channel\nRequires=sysinit.target\nWants=dbus.service systemd-logind.service\nAfter=sysinit.target dbus.service systemd-logind.service\nBefore=shutdown.target\nConflicts=shutdown.target\n[Service]\nType=notify\nEnvironment=NODEHARBOR_CONTROL_UNIT_REVISION=$GUEST_CONTROL_REVISION\nStandardOutput=journal+console\nExecStart=/usr/bin/python3 /usr/local/lib/nodeharbor/android_control.py\nRestart=on-failure\nRestartSec=5\nTimeoutStartSec=120\nTimeoutStopSec=10\n[Install]\nWantedBy=multi-user.target\n", "0644")
        val config = JSONObject().put("users", JSONArray()).put("disable_root", true).put("ssh_pwauth", false)
            .put("package_update", false).put("package_upgrade", false).put("write_files", writes)
            .put("runcmd", JSONArray().put(JSONArray(listOf("resize2fs", "/dev/vda")))
                .put(JSONArray(listOf("systemctl", "daemon-reload")))
                .put(JSONArray(listOf("systemctl", "reenable", "nodeharbor-control.service", "nodeharbor-watchdog.timer")))
                .put(JSONArray(listOf("systemctl", "restart", "nodeharbor-control.service")))
                .put(JSONArray(listOf("systemctl", "start", "nodeharbor-watchdog.timer"))))
        return seedImage(mapOf(
            "meta-data" to "instance-id: nodeharbor-${receipt.generation}-${BuildConfig.VERSION_CODE}-control-$GUEST_CONTROL_REVISION\nlocal-hostname: nodeharbor-worker\n".toByteArray(),
            "user-data" to "#cloud-config\n$config\n".toByteArray(),
            "network-config" to "version: 2\nethernets:\n  worker:\n    match:\n      name: 'e*'\n    dhcp4: true\n    dhcp6: false\n    nameservers:\n      addresses: [10.0.2.3]\n".toByteArray()
        ))
    }
}
