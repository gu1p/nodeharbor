package io.github.gu1p.nodeharbor

import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import org.json.JSONObject
import java.io.Closeable
import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock

/** Bounded framing over an already-connected private socket; no network listener. */
class GuestChannel(private val descriptor: ParcelFileDescriptor, private val owner: String) : Closeable {
    private val closed = AtomicBoolean(false)
    private var sequence = 0L
    private val commandLock = ReentrantLock(true)
    init {
        val flags = Os.fcntlInt(descriptor.fileDescriptor, OsConstants.F_GETFL, 0)
        Os.fcntlInt(descriptor.fileDescriptor, OsConstants.F_SETFL, flags or OsConstants.O_NONBLOCK)
    }
    fun request(command: GuestCommand, bootstrap: JSONObject? = null, timeoutMillis: Long = 15_000): JSONObject {
        require(timeoutMillis in 1..1_200_000)
        val deadline = SystemClock.elapsedRealtime() + timeoutMillis
        check(!closed.get()) { "The private guest channel is closed" }
        check(commandLock.tryLock(timeoutMillis, TimeUnit.MILLISECONDS)) { "The guest command expired while waiting for its channel" }
        try {
        check(!closed.get()) { "The private guest channel is closed" }
        val id = ++sequence
        val body = guestRequest(id, owner, command, bootstrap)
        transfer(ByteBuffer.allocate(body.size + 4).putInt(body.size).put(body).array(), true, deadline)
        val length = ByteBuffer.wrap(transfer(ByteArray(4), false, deadline)).int
        require(length in 2..1024 * 1024) { "The guest returned an invalid frame size" }
        return guestResponse(transfer(ByteArray(length), false, deadline), id, owner)
        } catch (error: Exception) {
            // A partial request/reply cannot safely be interpreted as a later
            // frame. Revoke this channel and let supervision stop the guest.
            close()
            throw error
        } finally { commandLock.unlock() }
    }
    private fun transfer(bytes: ByteArray, write: Boolean, deadline: Long): ByteArray {
        var offset = 0
        while (offset < bytes.size) {
            check(!closed.get()) { "The private guest channel is closed" }
            val remaining = deadline - SystemClock.elapsedRealtime()
            check(remaining > 0) { "The guest did not respond before its deadline" }
            val poll = StructPollfd().apply {
                fd = descriptor.fileDescriptor
                events = (if (write) OsConstants.POLLOUT else OsConstants.POLLIN).toShort()
            }
            try {
                check(Os.poll(arrayOf(poll), minOf(remaining, 1000).toInt()) >= 0)
                if (poll.revents.toInt() == 0) continue
                val count = if (write) Os.write(descriptor.fileDescriptor, bytes, offset, bytes.size - offset)
                    else Os.read(descriptor.fileDescriptor, bytes, offset, bytes.size - offset)
                check(count > 0) { "The private guest channel closed" }
                offset += count
            } catch (error: ErrnoException) {
                if (error.errno != OsConstants.EAGAIN && error.errno != OsConstants.EINTR)
                    throw IllegalStateException("The private guest channel is unavailable", error)
            }
        }
        return bytes
    }
    override fun close() {
        if (closed.compareAndSet(false, true)) {
            runCatching { Os.shutdown(descriptor.fileDescriptor, OsConstants.SHUT_RDWR) }
            runCatching { descriptor.close() }
        }
    }
}
