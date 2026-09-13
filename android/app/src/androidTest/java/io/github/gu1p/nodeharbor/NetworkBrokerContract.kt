package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import java.net.Inet4Address
import java.net.InetAddress
import org.junit.Assert.*
import org.junit.Test

class NetworkBrokerContract {
    @Test fun isolatedGuestUsesAnOwnerControlledStreamWithoutNetworkPermission() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val address = InetAddress.getAllByName("example.com").first { it is Inet4Address }.address
        val result = SandboxClient(context).networkProbe(address)
        assertTrue("The isolated service must receive a real HTTP response through the broker", result.startsWith("HTTP/"))
    }
    @Test fun closingTheOwnerBrokerRevokesItsCapabilities() {
        val broker = NetworkBroker(InstrumentationRegistry.getInstrumentation().targetContext) { true }
        broker.close()
        assertThrows(IllegalStateException::class.java) { broker.openUdp(7) }
    }
}
