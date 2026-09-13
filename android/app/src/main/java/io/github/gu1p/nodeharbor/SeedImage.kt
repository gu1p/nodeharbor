package io.github.gu1p.nodeharbor

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Locale

/** A small ISO 9660 NoCloud volume. Never mounts a host filesystem or runs tools. */
fun seedImage(files: Map<String, ByteArray>): ByteArray {
    require(files.keys == setOf("meta-data", "user-data", "network-config")) { "Invalid worker seed files" }
    require(files.values.sumOf { it.size.toLong() } <= 1024 * 1024) { "Worker seed exceeds its supported size" }
    val entries = files.toSortedMap()
    val block = 2048
    val sectors = 21 + entries.values.sumOf { (it.size + block - 1) / block }
    val bytes = ByteArray(sectors * block)
    fun number(offset: Int, value: Int, width: Int, order: ByteOrder) {
        val buffer = ByteBuffer.wrap(bytes, offset, width).order(order)
        if (width == 2) buffer.putShort(value.toShort()) else buffer.putInt(value)
    }
    fun both(offset: Int, value: Int, width: Int) {
        number(offset, value, width, ByteOrder.LITTLE_ENDIAN)
        number(offset + width, value, width, ByteOrder.BIG_ENDIAN)
    }
    fun text(offset: Int, length: Int, value: String) {
        bytes.fill(32, offset, offset + length)
        value.toByteArray(Charsets.US_ASCII).copyInto(bytes, offset)
    }
    fun record(offset: Int, extent: Int, size: Int, name: ByteArray, directory: Boolean): Int {
        val length = 33 + name.size + if (name.size % 2 == 0) 1 else 0
        bytes[offset] = length.toByte()
        both(offset + 2, extent, 4); both(offset + 10, size, 4)
        byteArrayOf(126, 1, 1, 0, 0, 0, 0).copyInto(bytes, offset + 18)
        bytes[offset + 25] = if (directory) 2 else 0
        both(offset + 28, 1, 2)
        bytes[offset + 32] = name.size.toByte()
        name.copyInto(bytes, offset + 33)
        return length
    }
    val pvd = 16 * block
    bytes[pvd] = 1; text(pvd + 1, 5, "CD001"); bytes[pvd + 6] = 1
    text(pvd + 8, 32, "LINUX"); text(pvd + 40, 32, "CIDATA")
    both(pvd + 80, sectors, 4); both(pvd + 120, 1, 2); both(pvd + 124, 1, 2)
    both(pvd + 128, block, 2); both(pvd + 132, 10, 4)
    number(pvd + 140, 18, 4, ByteOrder.LITTLE_ENDIAN); number(pvd + 148, 19, 4, ByteOrder.BIG_ENDIAN)
    record(pvd + 156, 20, block, byteArrayOf(0), true)
    text(pvd + 190, 128, "NODEHARBOR"); text(pvd + 318, 128, "NODEHARBOR")
    text(pvd + 446, 128, "NODEHARBOR"); text(pvd + 574, 128, "NODEHARBOR")
    for (offset in listOf(813, 830, 847, 864)) text(pvd + offset, 16, "2026010100000000")
    bytes[pvd + 881] = 1
    bytes[17 * block] = 255.toByte(); text(17 * block + 1, 5, "CD001"); bytes[17 * block + 6] = 1
    for ((sector, order) in listOf(18 to ByteOrder.LITTLE_ENDIAN, 19 to ByteOrder.BIG_ENDIAN)) {
        bytes[sector * block] = 1
        number(sector * block + 2, 20, 4, order); number(sector * block + 6, 1, 2, order)
    }
    var directory = 20 * block
    directory += record(directory, 20, block, byteArrayOf(0), true)
    directory += record(directory, 20, block, byteArrayOf(1), true)
    var sector = 21
    for ((name, data) in entries) {
        directory += record(directory, sector, data.size, "${name.uppercase(Locale.ROOT)};1".toByteArray(Charsets.US_ASCII), false)
        data.copyInto(bytes, sector * block)
        sector += (data.size + block - 1) / block
    }
    return bytes
}
