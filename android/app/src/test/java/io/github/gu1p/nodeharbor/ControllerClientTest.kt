package io.github.gu1p.nodeharbor

import java.io.ByteArrayInputStream
import org.junit.Assert.*
import org.junit.Test

class ControllerClientTest {
    @Test fun endpointsKeepCredentialsOnTheConfiguredHttpsOrigin() {
        assertEquals("https://fleet.example/path", controllerAddress(" https://fleet.example/path/ "))
        listOf("http://fleet.example", "https://user:secret@fleet.example", "https://fleet.example/?token=x",
            "https://fleet.example/#x", "https://", "https://fleet.example/../other").forEach { address ->
            assertThrows(address, IllegalArgumentException::class.java) { controllerAddress(address) }
        }
    }
    @Test fun remoteErrorsAndOversizedBodiesNeverExposeSecretsOrBecomeSuccess() {
        assertThrows(ControllerFailure::class.java) { checkedControllerResponse(302, ByteArrayInputStream("redirect secret".toByteArray())) }
        val error = assertThrows(ControllerFailure::class.java) { checkedControllerResponse(401, ByteArrayInputStream("credential secret".toByteArray())) }
        assertEquals(401, error.status)
        assertFalse(error.message!!.contains("secret"))
        assertThrows(IllegalArgumentException::class.java) { checkedControllerResponse(200, ByteArrayInputStream(ByteArray(1024 * 1024 + 1))) }
        assertEquals("{}", checkedControllerResponse(200, ByteArrayInputStream("{}".toByteArray())))
    }
}
