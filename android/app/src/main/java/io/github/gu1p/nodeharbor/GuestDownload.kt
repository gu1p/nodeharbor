package io.github.gu1p.nodeharbor

import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

/** The verified archive can write exactly one pinned image, never archive paths. */
fun extractGuestImage(input: InputStream, output: OutputStream, entry: String, imageBytes: Long,
                      imageSha256: String, active: () -> Boolean) {
    require(imageBytes in 1..(8L * 1024 * 1024 * 1024) && imageSha256.matches(Regex("[0-9a-f]{64}")))
    var found = false
    var entries = 0
    val buffer = ByteArray(1024 * 1024)
    fun readExactly(size: Int): ByteArray {
        val bytes = input.readNBytes(size)
        require(bytes.size == size) { "The guest archive is truncated" }
        return bytes
    }
    while (true) {
        check(active()) { "Worker preparation was stopped by your sharing rules" }
        val header = readExactly(512)
        if (header.all { it == 0.toByte() }) {
            require(readExactly(512).all { it == 0.toByte() }) { "Invalid guest archive ending" }
            break
        }
        require(++entries <= 8) { "Unexpected guest archive entries" }
        fun field(offset: Int, size: Int) = header.copyOfRange(offset, offset + size).toString(Charsets.US_ASCII).substringBefore('\u0000').trim()
        fun octal(offset: Int, size: Int) = field(offset, size).toLongOrNull(8)
            ?: throw IllegalArgumentException("Invalid guest archive size")
        val checksum = octal(148, 8)
        require(checksum == header.mapIndexed { index, value -> if (index in 148..155) 32 else value.toInt() and 255 }.sum().toLong()) {
            "Invalid guest archive header checksum"
        }
        val name = field(0, 100)
        val size = octal(124, 12)
        require((header[156] == 0.toByte() || header[156] == '0'.code.toByte()) && field(345, 155).isEmpty()) {
            "The guest archive contains an unsupported entry"
        }
        require((name == entry && !found && size == imageBytes) || (name == "README" && size in 0..65536)) {
            "The guest archive does not match its pinned disk"
        }
        val hash = MessageDigest.getInstance("SHA-256")
        var remaining = size
        while (remaining > 0) {
            check(active()) { "Worker preparation was stopped by your sharing rules" }
            val count = input.read(buffer, 0, minOf(buffer.size.toLong(), remaining).toInt())
            require(count > 0) { "The guest archive is truncated" }
            if (name == entry) { output.write(buffer, 0, count); hash.update(buffer, 0, count) }
            remaining -= count
        }
        if (name == entry) {
            require(hash.digest().joinToString("") { "%02x".format(it) } == imageSha256) { "The guest disk failed SHA-256 verification" }
            found = true
        }
        readExactly(((512 - size % 512) % 512).toInt())
    }
    require(found) { "The guest archive contains no worker disk" }
    output.flush()
}
