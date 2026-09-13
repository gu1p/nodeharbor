package io.github.gu1p.nodeharbor

import java.net.InetAddress
import java.net.InetSocketAddress
import java.nio.ByteBuffer

/** Only this virtual endpoint delegates DNS selection to Android. */
fun isGuestDns(address: ByteArray, port: Int): Boolean =
    port == 53 && address.contentEquals(byteArrayOf(10, 0, 2, 3))

fun brokerDestination(address: ByteArray, port: Int): InetSocketAddress {
    require(address.size == 4 && port in 1..65535) { "Unsupported guest network destination" }
    val first = address[0].toInt() and 255
    val second = address[1].toInt() and 255
    require(first !in listOf(0, 127) && first < 224 && !(first == 169 && second == 254)) {
        "The guest cannot connect to phone loopback, link-local, or multicast addresses"
    }
    return InetSocketAddress(InetAddress.getByAddress(address.copyOf()), port)
}

fun datagramEnvelope(address: ByteArray, port: Int, payload: ByteArray): ByteArray {
    brokerDestination(address, port)
    require(payload.size <= 65507) { "Guest datagram exceeds its supported size" }
    return ByteBuffer.allocate(12 + payload.size).putInt(payload.size).put(address)
        .putShort(port.toShort()).putShort(0).put(payload).array()
}
