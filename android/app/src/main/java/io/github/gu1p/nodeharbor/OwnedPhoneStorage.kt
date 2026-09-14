package io.github.gu1p.nodeharbor

import android.content.Context
import android.system.Os
import android.util.AtomicFile
import java.io.Closeable
import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardOpenOption
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/** Private journals authorize exact app-scoped files; a missing volume is never replaced implicitly. */
class OwnedPhoneStorage(context: Context, private val directory: File = context.noBackupFilesDir.resolve("nodeharbor/storage")) {
    val locations = PhoneStorageLocations(context)
    private val identity = directory.canonicalPath
    private val metadata = AtomicFile(directory.resolve("pool.json"))
    private fun reserve(): Closeable {
        val token = Any()
        check(reservations.putIfAbsent(identity, token) == null) { "Wait for confirmed worker shutdown before changing its storage" }
        return Closeable { reservations.remove(identity, token) }
    }
    fun load(owner: String): PhoneStorageState = synchronized(metadataLock) {
        require(UUID.fromString(owner).toString() == owner)
        check(!Files.isSymbolicLink(directory.toPath()) && !Files.isSymbolicLink(metadata.baseFile.toPath())) { "Unexpected storage journal link; files were preserved" }
        if (!metadata.baseFile.exists() && !directory.resolve("pool.json.bak").exists()) PhoneStorageState(owner)
        else PhoneStorageState.decode(metadata.openRead().use { it.readNBytes(1024 * 1024 + 1).toString(Charsets.UTF_8) }, owner)
    }
    private fun save(state: PhoneStorageState) = synchronized(metadataLock) {
        val encoded = state.encode()
        PhoneStorageState.decode(encoded, state.deviceId)
        check(directory.isDirectory || directory.mkdirs()) { "Private storage journal is unavailable" }
        val output = metadata.startWrite()
        try { output.write(encoded.toByteArray()); metadata.finishWrite(output) }
        catch (error: Exception) { metadata.failWrite(output); throw error }
    }
    fun review(owner: String, target: List<PhoneStorageDisk>): PhoneStorageReview {
        val state = load(owner)
        check(state.pending == null) { "Finish the pending storage change first" }
        check(state.retained.size < 240) { "Review retained storage copies before another change" }
        return reviewPhoneStorage(state.pool, target, locations.snapshot()).copy(generation = Math.addExact(state.generation, 1))
    }
    fun begin(owner: String, review: PhoneStorageReview, confirmedStopped: Boolean): PhoneStorageChange = reserve().use {
        check(confirmedStopped) { "Wait for Android to confirm worker shutdown" }
        val state = load(owner)
        check(state.pending == null && review.generation == Math.addExact(state.generation, 1)) { "The reviewed storage changed; review it again" }
        val original = state.pool?.disks.orEmpty().associateBy { it.id }
        val disks = review.target.map { disk ->
            val old = original[disk.id]
            if (!review.requiresBackup && old != null && old.location == disk.location && old.gib == disk.gib) old
            else disk.copy(incarnation = UUID.randomUUID().toString())
        }
        val target = PhoneStoragePool(owner, if (review.requiresBackup) UUID.randomUUID().toString() else state.pool?.poolId ?: UUID.randomUUID().toString(),
            review.generation, disks).also { it.validate(owner) }
        val change = PhoneStorageChange(UUID.randomUUID().toString(), state.pool, target, review.requiresBackup, "planned")
        save(state.copy(pending = change))
        change
    }
    private fun file(owner: String, disk: PhoneStorageDisk): File {
        disk.validate()
        require(UUID.fromString(owner).toString() == owner)
        val folder = locations.directory(disk.location).resolve(owner)
        check(!Files.isSymbolicLink(folder.toPath())) { "Unexpected storage owner link; files were preserved" }
        check(folder.isDirectory || folder.mkdirs()) { "Selected owner storage is unavailable" }
        return folder.resolve(disk.filename).also {
            check(!Files.isSymbolicLink(it.toPath())) { "Unexpected storage member link; files were preserved" }
        }
    }
    private fun checkedFile(owner: String, disk: PhoneStorageDisk): File = file(owner, disk).also {
        check(it.isFile && it.length() == disk.bytes) { "A selected storage member is missing or has changed size. The whole worker is unavailable." }
    }
    fun unavailable(owner: String): Boolean = runCatching {
        val state = load(owner)
        state.pool?.disks?.forEach { checkedFile(owner, it) }
        false
    }.getOrDefault(true)
    fun reserveForVm(owner: String, maintenance: Boolean = false): Closeable {
        val reservation = reserve()
        try {
            vmFiles(owner, maintenance)
            return reservation
        } catch (error: Exception) { reservation.close(); throw error }
    }
    fun vmFiles(owner: String, maintenance: Boolean = false): List<File> {
        val state = load(owner)
        check(!state.disabled || maintenance) { "Worker storage is disabled. Review new storage before enabling sharing." }
        check(maintenance || state.recovery == null) { "Finish the pending whole-pool recovery before starting the worker" }
        val pool = if (state.pending == null) state.pool else {
            check(maintenance) { "Finish the pending storage change before starting the worker" }
            if (state.pending.phase in setOf("files-ready", "verified")) state.pending.target else state.pending.previous
        }
        return pool?.disks.orEmpty().map { checkedFile(owner, it) }
    }
    fun markBackedUp(owner: String, id: String, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped)
        val state = load(owner); val change = checkNotNull(state.pending)
        check(change.id == id && change.phase == "planned" && change.requiresBackup)
        save(state.copy(pending = change.copy(phase = "backed-up")))
    }
    fun prepare(owner: String, confirmedStopped: Boolean, active: () -> Boolean, progress: (String) -> Unit) = reserve().use {
        check(confirmedStopped) { "Wait for Android to confirm worker shutdown" }
        val state = load(owner); val change = checkNotNull(state.pending)
        if (change.phase in setOf("files-ready", "verified")) return@use
        check(change.phase == "backed-up" || change.phase == "planned" && !change.requiresBackup) { "Verify the whole-pool backup before replacing storage" }
        // Remove only this journal's unfinished copies before recalculating free
        // capacity; otherwise a crash after allocation could prevent its own retry.
        for (disk in change.target.disks) {
            check(active()) { "Storage preparation was stopped by the owner" }
            if (change.previous?.disks?.contains(disk) == true) continue
            val partial = file(owner, disk)
            if (partial.exists()) check(partial.isFile && partial.length() <= disk.bytes && partial.delete()) { "An unfinished owned copy changed; files were preserved" }
        }
        // Capacity may have changed after review. Preserve all originals on failure.
        reviewPhoneStorage(change.previous, change.target.disks, locations.snapshot())
        for (disk in change.target.disks) {
            check(active()) { "Storage preparation was stopped by the owner" }
            val old = change.previous?.disks?.find { it.id == disk.id }
            if (old == disk) { checkedFile(owner, disk); continue }
            val target = file(owner, disk)
            // Only this durable journal's uncommitted copies can be retried.
            if (target.exists()) check(target.isFile && target.delete()) { "An unfinished owned copy could not be removed" }
            progress("Reserving and verifying ${disk.gib} GiB on selected app storage")
            if (old != null && !change.requiresBackup) copyVerifiedStorage(checkedFile(owner, old), target, disk.bytes, active)
            else {
                Files.newByteChannel(target.toPath(), StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE).close()
                try {
                    RandomAccessFile(target, "rw").use { image ->
                        image.setLength(disk.bytes); Os.posix_fallocate(image.fd, 0, disk.bytes); image.fd.sync()
                    }
                    check(active()) { "Storage preparation was stopped by the owner" }
                } catch (error: Exception) { target.delete(); throw error }
            }
        }
        save(state.copy(pending = change.copy(phase = "files-ready")))
    }
    fun markVerified(owner: String, id: String, generation: Long, poolId: String, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped)
        val state = load(owner); val change = checkNotNull(state.pending)
        check(change.id == id && change.phase in setOf("files-ready", "verified") &&
            change.target.generation == generation && change.target.poolId == poolId) { "Guest storage verification did not match the reviewed pool" }
        change.target.disks.forEach { checkedFile(owner, it) }
        save(state.copy(pending = change.copy(phase = "verified")))
    }
    fun commit(owner: String, confirmedStopped: Boolean) = reserve().use { save(load(owner).commit(confirmedStopped)) }
    fun recoveryPreference(owner: String, enabled: Boolean) = synchronized(metadataLock) { save(load(owner).copy(automaticRecovery = enabled)) }
    fun beginRecovery(owner: String, replacement: List<PhoneStorageDisk>, confirmedStopped: Boolean): PhoneStorageRecovery = reserve().use {
        check(confirmedStopped)
        val state = load(owner)
        check(state.automaticRecovery && state.pool != null && state.pending == null && state.recovery == null) { "Whole-pool recovery has not been authorized" }
        val recovery = PhoneStorageRecovery(UUID.randomUUID().toString(), replacement, "resetting")
        save(state.copy(recovery = recovery))
        recovery
    }
    fun recoveryRebuilding(owner: String, operation: String, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped)
        val state = load(owner); val recovery = checkNotNull(state.recovery)
        check(recovery.operation == operation && state.pool == null && state.pending == null && state.disabled)
        save(state.copy(recovery = recovery.copy(phase = "rebuilding")))
    }
    fun finishRecovery(owner: String, operation: String, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped)
        val state = load(owner); val recovery = checkNotNull(state.recovery)
        check(recovery.operation == operation && recovery.phase == "rebuilding" && state.pool != null && state.pending == null)
        check(state.pool.disks.map { it.location to it.gib } == recovery.replacement.map { it.location to it.gib })
        save(state.copy(recovery = null))
    }
    fun allocatedBytes(owner: String): Long {
        if (owner.isEmpty()) return 0
        val state = load(owner)
        return (state.pool?.disks.orEmpty() + state.pending?.target?.disks.orEmpty() + state.retained)
            .distinctBy { it.location to it.filename }.sumOf { disk ->
                runCatching { Os.stat(file(owner, disk).absolutePath).st_blocks * 512 }.getOrDefault(0)
            }
    }
    fun recoveryCredits(owner: String): Map<String, Long> {
        val result = mutableMapOf<String, Long>()
        val seen = mutableSetOf<Pair<Long, Long>>()
        for (disk in load(owner).pool?.disks.orEmpty()) runCatching {
            val stat = Os.stat(checkedFile(owner, disk).absolutePath)
            if (seen.add(stat.st_dev to stat.st_ino)) {
                val filesystem = stat.st_dev.toString()
                result[filesystem] = (result[filesystem] ?: 0) + (stat.st_blocks * 512).coerceIn(0, disk.bytes)
            }
        }
        return result
    }
    fun cleanupRetained(owner: String, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped) { "Wait for Android to confirm worker shutdown" }
        var state = load(owner)
        check(state.pending == null) { "Finish the pending storage change before removing retained copies" }
        for (disk in state.retained) {
            check(state.pool?.disks?.none { it.location == disk.location && it.filename == disk.filename } != false)
            val target = file(owner, disk)
            if (target.exists()) check(target.isFile && target.length() == disk.bytes && target.delete()) { "The retained storage copy changed; files were preserved" }
            state = state.copy(retained = state.retained - disk); save(state)
        }
    }
    fun forgetBackups(owner: String, completed: List<PhoneStorageBackup>, confirmedStopped: Boolean) = reserve().use {
        check(confirmedStopped)
        val state = load(owner)
        check(state.pending == null && state.backups.containsAll(completed))
        save(state.copy(backups = state.backups - completed.toSet()))
    }
    fun removeAll(owner: String, confirmedStopped: Boolean, resetConfirmed: Boolean, disabled: Boolean,
                  preserveRecovery: Boolean = false, active: () -> Boolean = { true }) = reserve().use {
        check(confirmedStopped && resetConfirmed) { "Confirm Android process death and fleet access removal before deleting worker storage" }
        val state = load(owner)
        val disks = (state.pool?.disks.orEmpty() + state.pending?.target?.disks.orEmpty() + state.retained)
            .distinctBy { it.location to it.filename }
        val unavailable = mutableListOf<PhoneStorageDisk>()
        for (disk in disks) {
            check(active()) { "Storage deletion was stopped by the owner" }
            val target = try { file(owner, disk) } catch (_: Exception) { unavailable += disk; continue }
            if (target.exists()) {
                val temporary = state.pending?.target?.disks?.contains(disk) == true && state.pool?.disks?.contains(disk) != true
                check(target.isFile && (target.length() == disk.bytes || temporary && target.length() <= disk.bytes) && target.delete()) {
                    "An owned storage file changed or could not be removed; remaining files were preserved"
                }
            }
        }
        save(state.copy(pool = null, pending = null, generation = if (state.pool == null && state.pending == null && state.disabled == disabled) state.generation
            else Math.addExact(state.pending?.target?.generation ?: state.generation, 1),
            retained = unavailable, disabled = disabled, backups = emptyList(), recovery = if (preserveRecovery) state.recovery else null))
    }
    companion object {
        private val reservations = ConcurrentHashMap<String, Any>()
        private val metadataLock = Any()
    }
}
