package io.github.gu1p.nodeharbor

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class GuestProtocolTest {
    private val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
    @Test fun onlyFixedCommandsAndOwnerBoundResponsesCrossTheGuestBoundary() {
        val request = guestRequest(1, owner, GuestCommand.Status)
        assertEquals("status", JSONObject(request.toString(Charsets.UTF_8)).getString("command"))
        val reply = JSONObject().put("id", 1).put("deviceId", owner).put("ok", true).toString().toByteArray()
        assertTrue(guestResponse(reply, 1, owner).getBoolean("ok"))
        assertThrows(IllegalArgumentException::class.java) { guestResponse(reply, 2, owner) }
        assertThrows(IllegalArgumentException::class.java) { guestResponse(reply, 1, "different") }
        assertThrows(IllegalArgumentException::class.java) { guestResponse(ByteArray(1024 * 1024 + 1), 1, owner) }
    }
    @Test fun bootstrapRequiresTheSameOwnerAndCannotBeSentWithAnotherCommand() {
        val grant = JSONObject().put("deviceId", owner)
        assertTrue(guestRequest(1, owner, GuestCommand.Configure, grant).isNotEmpty())
        assertThrows(IllegalArgumentException::class.java) { guestRequest(1, owner, GuestCommand.Status, grant) }
        assertThrows(IllegalArgumentException::class.java) { guestRequest(1, owner, GuestCommand.Configure, JSONObject().put("deviceId", "different")) }
    }
    @Test fun guestResponsesCannotAddTrailingObjectsOrExcessiveNesting() {
        val base = JSONObject().put("id", 1).put("deviceId", owner).put("ok", true).toString()
        assertThrows(IllegalArgumentException::class.java) { guestResponse((base + "{}").toByteArray(), 1, owner) }
        val nested = base.dropLast(1) + ",\"extra\":" + "[".repeat(32) + "0" + "]".repeat(32) + "}"
        assertThrows(IllegalArgumentException::class.java) { guestResponse(nested.toByteArray(), 1, owner) }
    }
}
