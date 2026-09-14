package io.github.gu1p.nodeharbor

import android.content.Context
import android.os.Environment
import android.os.storage.StorageManager
import android.system.Os
import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardOpenOption
import java.security.MessageDigest

/** Only platform-provided, app-specific directories can supply block-device files. */
class PhoneStorageLocations(private val context: Context) {
    private fun entries(): List<Triple<String, String, File>> {
        val manager = context.getSystemService(StorageManager::class.java)
        val result = mutableListOf(Triple("internal", "Internal storage", context.noBackupFilesDir.resolve("nodeharbor/storage-disks")))
        for (root in context.getExternalFilesDirs(null).filterNotNull()) {
            val volume = manager.getStorageVolume(root) ?: continue
            val id = if (volume.isPrimary) "primary" else volume.uuid?.takeIf { it.matches(Regex("[a-zA-Z0-9_-]{1,64}")) }?.let { "volume-$it" } ?: continue
            result.add(Triple(id, volume.getDescription(context).take(120), root.resolve("nodeharbor/storage-disks")))
        }
        return result.distinctBy { it.first }
    }
    fun directory(id: String): File {
        val entry = entries().singleOrNull { it.first == id }
        require(entry != null) { "Selected app storage is unavailable; no replacement location was selected" }
        val directory = entry.third
        check(!Files.isSymbolicLink(directory.toPath()) && !Files.isSymbolicLink(directory.parentFile!!.toPath())) { "Unexpected storage link; files were preserved" }
        check(id == "internal" || Environment.getExternalStorageState(directory) == Environment.MEDIA_MOUNTED) { "Selected storage is unavailable or read-only" }
        check(directory.isDirectory || directory.mkdirs()) { "Selected app storage is unavailable" }
        return directory
    }
    fun snapshot(): List<PhoneStorageLocation> = entries().map { (id, label, _) ->
        runCatching {
            val directory = directory(id)
            // Review capacity without relying on Android evicting another app's cache.
            PhoneStorageLocation(id, label, Os.stat(directory.absolutePath).st_dev.toString(), android.os.StatFs(directory.absolutePath).availableBytes, true,
                id != "internal" && Environment.isExternalStorageRemovable(directory))
        }.getOrElse { PhoneStorageLocation(id, label, "", 0, false, id != "internal") }
    }
}

/** The original stays intact until the caller commits a durable, verified receipt. */
fun copyVerifiedStorage(source: File, target: File, size: Long, active: () -> Boolean) {
    check(!Files.isSymbolicLink(source.toPath()) && source.isFile && !target.exists()) { "Unexpected storage file; originals were preserved" }
    val length = source.length()
    require(size >= length && size <= 65536 * STORAGE_GIB) { "A disk copy cannot truncate worker data" }
    check(active()) { "Storage copying was cancelled" }
    // CREATE_NEW never follows or overwrites a returned stale member.
    Files.newByteChannel(target.toPath(), StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE).close()
    try {
        val expected = MessageDigest.getInstance("SHA-256")
        RandomAccessFile(target, "rw").use { output ->
            output.setLength(size)
            Os.posix_fallocate(output.fd, 0, size)
            source.inputStream().use { input ->
                val buffer = ByteArray(1024 * 1024)
                var copied = 0L
                while (copied < length) {
                    check(active()) { "Storage copying was cancelled" }
                    val count = input.read(buffer, 0, minOf(buffer.size.toLong(), length - copied).toInt())
                    check(count > 0) { "The source disk changed while copying" }
                    output.write(buffer, 0, count); expected.update(buffer, 0, count); copied += count
                }
                check(input.read() == -1 && source.length() == length) { "The source disk changed while copying" }
            }
            output.fd.sync()
        }
        val actual = MessageDigest.getInstance("SHA-256")
        target.inputStream().use { input ->
            val buffer = ByteArray(1024 * 1024)
            var remaining = length
            while (remaining > 0) {
                check(active()) { "Storage verification was cancelled" }
                val count = input.read(buffer, 0, minOf(buffer.size.toLong(), remaining).toInt())
                check(count > 0) { "The copied disk is incomplete" }
                actual.update(buffer, 0, count); remaining -= count
            }
        }
        check(MessageDigest.isEqual(expected.digest(), actual.digest())) { "The copied disk failed verification; its original was preserved" }
    } catch (error: Exception) { target.delete(); throw error }
}
