package io.github.gu1p.nodeharbor

import android.os.ParcelFileDescriptor
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class GuestChannelContract {
    @Test fun waitingBehindAStartupCommandConsumesTheStopCommandDeadline() {
        val pair = ParcelFileDescriptor.createSocketPair()
        val channel = GuestChannel(pair[0], "9511182e-9c48-4d20-a15b-1da8bb441386")
        val threads = Executors.newFixedThreadPool(2)
        try {
            val startup = threads.submit { runCatching { channel.request(GuestCommand.Status, timeoutMillis = 10_000) } }
            // Receipt of its frame proves startup owns the command channel.
            ParcelFileDescriptor.AutoCloseInputStream(pair[1]).apply { read(ByteArray(4)) }
            val stop = threads.submit<Boolean> {
                try { channel.request(GuestCommand.Poweroff, timeoutMillis = 50); false }
                catch (_: IllegalStateException) { true }
            }
            assertTrue("Queueing must consume the caller's timeout", stop.get(1, TimeUnit.SECONDS))
            channel.close()
            startup.get(2, TimeUnit.SECONDS)
        } finally { channel.close(); pair[1].close(); threads.shutdownNow() }
    }

    @Test fun closingTheOwnerChannelInterruptsAnUnresponsiveGuest() {
        val pair = ParcelFileDescriptor.createSocketPair()
        val channel = GuestChannel(pair[0], "9511182e-9c48-4d20-a15b-1da8bb441386")
        val reader = Executors.newSingleThreadExecutor()
        try {
            val blocked = reader.submit<Boolean> {
                try { channel.request(GuestCommand.Status, timeoutMillis = 10_000); false }
                catch (_: IllegalStateException) { true }
            }
            channel.close()
            assertTrue("Closing the owner channel must interrupt a blocked guest request", blocked.get(2, TimeUnit.SECONDS))
        } finally { channel.close(); pair[1].close(); reader.shutdownNow() }
    }
    @Test fun anUnresponsiveGuestCannotKeepAnOwnerRequestOpen() {
        val pair = ParcelFileDescriptor.createSocketPair()
        try {
            GuestChannel(pair[0], "9511182e-9c48-4d20-a15b-1da8bb441386").use { channel ->
                assertThrows(IllegalStateException::class.java) { channel.request(GuestCommand.Status, timeoutMillis = 50) }
                val reused = assertThrows(IllegalStateException::class.java) { channel.request(GuestCommand.Status, timeoutMillis = 50) }
                assertTrue("A timed-out frame cannot be reused as a later owner reply", reused.message!!.contains("closed"))
            }
        } finally { pair[1].close() }
    }
}
