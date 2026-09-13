package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class NetworkBrokerTest {
    private fun ip(a: Int, b: Int = 0, c: Int = 0, d: Int = 1) = byteArrayOf(a.toByte(), b.toByte(), c.toByte(), d.toByte())
    @Test fun onlyTheGuestDnsEndpointUsesThePhonesSystemResolver() {
        assertTrue(isGuestDns(ip(10, 0, 2, 3), 53))
        assertFalse(isGuestDns(ip(10, 0, 2, 3), 443))
        assertFalse(isGuestDns(ip(8, 8, 8, 8), 53))
    }
    @Test fun guestNetworkCapabilitiesCannotTargetPhoneLoopbackOrSpecialAddresses() {
        for (address in listOf(ip(0), ip(127), ip(169, 254), ip(224), ip(255), ByteArray(16))) {
            assertThrows(IllegalArgumentException::class.java) { brokerDestination(address, 443) }
        }
        assertEquals("100.80.0.1", brokerDestination(ip(100, 80), 443).address.hostAddress)
        assertEquals("10.20.0.1", brokerDestination(ip(10, 20), 443).address.hostAddress)
        for (port in listOf(-1, 0, 65536)) assertThrows(IllegalArgumentException::class.java) { brokerDestination(ip(8, 8, 8, 8), port) }
    }
    @Test fun udpRelayPreservesTheSourceWithoutParsingGuestPacketsInTheOwnerProcess() {
        val frame = datagramEnvelope(ip(8, 8, 8, 8), 53, byteArrayOf(1, 2, 3))
        assertArrayEquals(byteArrayOf(0, 0, 0, 3, 8, 8, 8, 8, 0, 53, 0, 0, 1, 2, 3), frame)
        assertThrows(IllegalArgumentException::class.java) { datagramEnvelope(ip(8), 53, ByteArray(65508)) }
    }
}
