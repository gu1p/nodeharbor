package io.github.gu1p.nodeharbor

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.security.MessageDigest
import org.junit.Assert.*
import org.junit.Test

class GuestDownloadTest {
    private fun hash(value: ByteArray) = MessageDigest.getInstance("SHA-256").digest(value).joinToString("") { "%02x".format(it) }
    private fun tar(name: String, data: ByteArray, type: Char = '0'): ByteArray {
        val header = ByteArray(512)
        name.toByteArray().copyInto(header)
        (data.size.toString(8).padStart(11, '0') + "\u0000").toByteArray().copyInto(header, 124)
        header[156] = type.code.toByte()
        header.fill(32, 148, 156)
        val checksum = header.sumOf { it.toInt() and 255 }
        (checksum.toString(8).padStart(6, '0') + "\u0000 ").toByteArray().copyInto(header, 148)
        return header + data + ByteArray((512 - data.size % 512) % 512) + ByteArray(1024)
    }
    @Test fun verifiedGuestExtractionWritesOnlyThePinnedDiskBytes() {
        val data = "owned disk".toByteArray()
        val output = ByteArrayOutputStream()
        extractGuestImage(ByteArrayInputStream(tar("disk.img", data)), output, "disk.img", data.size.toLong(), hash(data)) { true }
        assertArrayEquals(data, output.toByteArray())
    }
    @Test fun wrongPathsLinksTruncationAndDigestMismatchNeverBecomeAWorker() {
        val data = "owned disk".toByteArray()
        for (archive in listOf(tar("../disk.img", data), tar("disk.img", data, '2'), tar("disk.img", data).copyOf(515), tar("disk.img", "bad image!".toByteArray()))) {
            assertThrows(IllegalArgumentException::class.java) {
                extractGuestImage(ByteArrayInputStream(archive), ByteArrayOutputStream(), "disk.img", data.size.toLong(), hash(data)) { true }
            }
        }
    }
    @Test fun revokingTheOwnerStopsGuestExtraction() {
        val data = "owned disk".toByteArray()
        assertThrows(IllegalStateException::class.java) {
            extractGuestImage(ByteArrayInputStream(tar("disk.img", data)), ByteArrayOutputStream(), "disk.img", data.size.toLong(), hash(data)) { false }
        }
    }
}
