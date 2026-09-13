package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertTrue
import org.junit.Test

class GuestNetworkContract {
    @Test fun guestPacketsReachTheCurrentNetworksDnsThroughTheIsolatedStack() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val dns = byteArrayOf(10, 0, 2, 3)
        assertTrue("The actual isolated guest network stack must receive a DNS reply", SandboxClient(context).guestNetworkProbe(dns))
    }
}
