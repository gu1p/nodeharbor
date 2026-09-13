package io.github.gu1p.nodeharbor

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.*
import org.junit.Test

class SeedImageTest {
    @Test fun seedIsAReadOnlyCidataVolumeWithExactCloudInitContents() {
        val content = mapOf("meta-data" to "instance-id: test\n".toByteArray(), "user-data" to "#cloud-config\n{}\n".toByteArray(),
            "network-config" to "version: 2\n".toByteArray())
        val image = seedImage(content)
        assertEquals("CD001", image.copyOfRange(16 * 2048 + 1, 16 * 2048 + 6).toString(Charsets.US_ASCII))
        assertEquals("CIDATA", image.copyOfRange(16 * 2048 + 40, 16 * 2048 + 72).toString(Charsets.US_ASCII).trim())
        fun little(offset: Int) = ByteBuffer.wrap(image, offset, 4).order(ByteOrder.LITTLE_ENDIAN).int
        val directory = little(16 * 2048 + 158) * 2048
        var offset = directory
        val extracted = mutableMapOf<String, ByteArray>()
        while (image[offset] != 0.toByte()) {
            val length = image[offset].toInt() and 255
            val nameLength = image[offset + 32].toInt() and 255
            if (image[offset + 25].toInt() and 2 == 0) {
                val name = image.copyOfRange(offset + 33, offset + 33 + nameLength).toString(Charsets.US_ASCII).substringBefore(';').lowercase()
                val start = little(offset + 2) * 2048
                extracted[name] = image.copyOfRange(start, start + little(offset + 10))
            }
            offset += length
        }
        assertEquals(content.keys, extracted.keys)
        content.forEach { (name, bytes) -> assertArrayEquals(bytes, extracted[name]) }
    }
    @Test fun seedHasNoArbitraryPathsAndBoundedContent() {
        assertThrows(IllegalArgumentException::class.java) { seedImage(mapOf("../escape" to byteArrayOf(1))) }
        assertThrows(IllegalArgumentException::class.java) { seedImage(mapOf("user-data" to ByteArray(1024 * 1024 + 1))) }
    }
}
