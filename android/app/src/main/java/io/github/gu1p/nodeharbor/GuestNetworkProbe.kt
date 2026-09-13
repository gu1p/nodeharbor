package io.github.gu1p.nodeharbor

import android.os.ParcelFileDescriptor
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.FileInputStream
import java.io.FileOutputStream
import java.nio.ByteBuffer

/** Runs within the isolated service and exercises the actual packet stack. */
internal fun guestNetworkProbe(broker: ISocketBroker, dns: ByteArray): Boolean {
    require(dns.size == 4)
    val sockets = ParcelFileDescriptor.createSocketPair()
    val nativeFd = sockets[1].detachFd()
    val worker = Thread({ NativeNetwork.run(nativeFd, dns, SocketBridge(broker)) }, "nodeharbor-packet-probe")
    worker.start()
    try {
        val output = DataOutputStream(FileOutputStream(sockets[0].fileDescriptor))
        val input = DataInputStream(FileInputStream(sockets[0].fileDescriptor))
        val guestMac = byteArrayOf(0x52, 0x54, 0, 0x12, 0x34, 0x56)
        val guestIp = byteArrayOf(10, 0, 2, 15)
        val gateway = byteArrayOf(10, 0, 2, 2)
        val arp = ByteBuffer.allocate(42).put(ByteArray(6) { 0xff.toByte() }).put(guestMac).putShort(0x0806)
            .putShort(1).putShort(0x0800).put(6).put(4).putShort(1).put(guestMac).put(guestIp).put(ByteArray(6)).put(gateway).array()
        output.writeInt(arp.size); output.write(arp); output.flush()
        val query = ByteBuffer.allocate(29).putShort(0x4e48).putShort(0x0100).putShort(1)
            .putShort(0).putShort(0).putShort(0).put(7).put("example".toByteArray()).put(3).put("com".toByteArray())
            .put(0).putShort(1).putShort(1).array()
        val ip = ByteBuffer.allocate(20).put(0x45).put(0).putShort((28 + query.size).toShort()).putShort(1)
            .putShort(0).put(64).put(17).putShort(0).put(guestIp).put(dns).array()
        var sum = 0
        for (index in ip.indices step 2) sum += ((ip[index].toInt() and 255) shl 8) or (ip[index + 1].toInt() and 255)
        while (sum > 65535) sum = (sum and 65535) + (sum ushr 16)
        val checksum = sum.inv() and 65535
        ip[10] = (checksum ushr 8).toByte(); ip[11] = checksum.toByte()
        val packet = ByteBuffer.allocate(14 + 28 + query.size).put(byteArrayOf(0x52, 0x55, 10, 0, 2, 2)).put(guestMac)
            .putShort(0x0800).put(ip).putShort(40000.toShort()).putShort(53).putShort((8 + query.size).toShort()).putShort(0).put(query).array()
        output.writeInt(packet.size); output.write(packet); output.flush()
        repeat(64) {
            val poll = android.system.StructPollfd().apply {
                fd = sockets[0].fileDescriptor
                events = android.system.OsConstants.POLLIN.toShort()
            }
            check(android.system.Os.poll(arrayOf(poll), 10_000) > 0) { "The guest DNS probe received no reply after $it frames" }
            val size = input.readInt()
            require(size in 14..65536) { "Invalid isolated network frame" }
            val frame = ByteArray(size).also(input::readFully)
            if (size >= 54 && frame[12] == 8.toByte() && frame[13] == 0.toByte() && frame[23] == 17.toByte()) {
                val body = 14 + (frame[14].toInt() and 15) * 4 + 8
                if (body + 12 <= size && frame[body] == 0x4e.toByte() && frame[body + 1] == 0x48.toByte() &&
                    (frame[body + 2].toInt() and 128) != 0 && (frame[body + 3].toInt() and 15) == 0) return true
            }
        }
        return false
    } finally {
        runCatching { android.system.Os.shutdown(sockets[0].fileDescriptor, android.system.OsConstants.SHUT_RDWR) }
        sockets[0].close()
        worker.join(1000)
    }
}
