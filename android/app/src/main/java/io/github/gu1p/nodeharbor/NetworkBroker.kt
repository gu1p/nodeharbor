package io.github.gu1p.nodeharbor

import android.net.DnsResolver
import android.os.Binder
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import java.io.Closeable
import java.io.FileDescriptor
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/** Relays bounded streams and datagrams. Ethernet/IP/TCP parsing stays isolated. */
class NetworkBroker(context: android.content.Context, private val allowed: () -> Boolean) : ISocketBroker.Stub(), Closeable {
    @Suppress("DEPRECATION")
    private val resolver = if (android.os.Build.VERSION.SDK_INT >= 37) DnsResolver(context, null) else DnsResolver.getInstance()
    private class Flow(val socket: Closeable, val channel: ParcelFileDescriptor) : Closeable {
        val closed = AtomicBoolean(false)
        val writeLock = Any()
        val queries = ConcurrentHashMap.newKeySet<Closeable>()
        override fun close() {
            if (closed.compareAndSet(false, true)) {
                queries.toList().forEach { it.close() }
                runCatching { socket.close() }
                runCatching { Os.shutdown(channel.fileDescriptor, OsConstants.SHUT_RDWR) }
                runCatching { channel.close() }
            }
        }
    }
    private val flows = ConcurrentHashMap<Int, Flow>()
    private val udpOpened = AtomicInteger()
    private val udpSent = AtomicInteger()
    private val udpReceived = AtomicInteger()
    private val dnsRequested = AtomicInteger()
    private val dnsErrors = AtomicInteger()
    private val dnsAnswers = AtomicInteger()
    private val streamErrors = AtomicInteger()
    fun diagnosticCounts(): String = "UDP opened=${udpOpened.get()}, sent=${udpSent.get()}, received=${udpReceived.get()}, DNS requested=${dnsRequested.get()}, answers=${dnsAnswers.get()}, errors=${dnsErrors.get()}, stream errors=${streamErrors.get()}, flows=${flows.size}"
    private val threads = Executors.newCachedThreadPool { task -> Thread(task, "nodeharbor-network").apply { isDaemon = true } }
    private val deadlines = Executors.newSingleThreadScheduledExecutor { task -> Thread(task, "nodeharbor-dns-timeout").apply { isDaemon = true } }
    private val dnsSlots = Semaphore(8)
    private var closed = false
    @Synchronized private fun register(handle: Int, flow: Flow) {
        check(!closed && allowed()) { "The owner has stopped guest networking" }
        require(handle in 3..65535) { "Invalid guest network capability" }
        check(flows.size < 64 && flows.putIfAbsent(handle, flow) == null) { "The guest reached its network connection limit" }
    }
    override fun openTcp(handle: Int, address: ByteArray, port: Int): ParcelFileDescriptor {
        val destination = brokerDestination(address, port)
        val socket = Socket()
        val pair = ParcelFileDescriptor.createSocketPair()
        val flow = Flow(socket, pair[0])
        try {
            register(handle, flow)
            socket.tcpNoDelay = true
            socket.connect(destination, 10_000)
            check(allowed() && !flow.closed.get()) { "The owner has stopped guest networking" }
            val remaining = AtomicInteger(2)
            fun complete() { if (remaining.decrementAndGet() == 0) closeFlow(handle) }
            threads.execute {
                try {
                    val buffer = ByteArray(16 * 1024)
                    while (allowed() && !flow.closed.get()) {
                        val count = Os.read(pair[0].fileDescriptor, buffer, 0, buffer.size)
                        if (count <= 0) break
                        socket.getOutputStream().write(buffer, 0, count)
                    }
                    runCatching { socket.shutdownOutput() }
                } catch (_: Exception) { closeFlow(handle) }
                finally { complete() }
            }
            threads.execute {
                try {
                    val buffer = ByteArray(16 * 1024)
                    while (allowed() && !flow.closed.get()) {
                        val count = socket.getInputStream().read(buffer)
                        if (count < 0) break
                        writeAll(pair[0].fileDescriptor, buffer, count)
                    }
                    runCatching { Os.shutdown(pair[0].fileDescriptor, OsConstants.SHUT_WR) }
                } catch (_: Exception) { closeFlow(handle) }
                finally { complete() }
            }
            return pair[1]
        } catch (error: Exception) {
            flows.remove(handle, flow); flow.close(); pair[1].close()
            throw IllegalStateException("The guest TCP connection could not be opened", error)
        }
    }
    override fun openUdp(handle: Int): ParcelFileDescriptor {
        val socket = DatagramSocket(null)
        val pair = ParcelFileDescriptor.createSocketPair()
        val flow = Flow(socket, pair[0])
        try {
            register(handle, flow)
            socket.bind(null)
            udpOpened.incrementAndGet()
            threads.execute {
                try {
                    val buffer = ByteArray(65507)
                    while (allowed() && !flow.closed.get()) {
                        val packet = DatagramPacket(buffer, buffer.size)
                        socket.receive(packet)
                        udpReceived.incrementAndGet()
                        val frame = runCatching { datagramEnvelope(packet.address.address, packet.port, buffer.copyOf(packet.length)) }.getOrNull()
                        if (frame != null) synchronized(flow.writeLock) {
                            writeAll(pair[0].fileDescriptor, frame, frame.size)
                        }
                    }
                } catch (_: Exception) { /* Closing the owner capability interrupts receive. */ }
                finally { closeFlow(handle) }
            }
            return pair[1]
        } catch (error: Exception) {
            flows.remove(handle, flow); flow.close(); pair[1].close()
            throw IllegalStateException("The guest UDP connection could not be opened", error)
        }
    }
    override fun sendUdp(handle: Int, address: ByteArray, port: Int, payload: ByteArray): Int {
        check(allowed()) { "The owner has stopped guest networking" }
        require(payload.size <= 65507) { "Guest datagram exceeds its supported size" }
        val destination = brokerDestination(address, port)
        val flow = checkNotNull(flows[handle]) { "The guest network capability has expired" }
        val socket = flow.socket as? DatagramSocket ?: error("The capability is not a datagram socket")
        check(!flow.closed.get()) { "The guest network capability has expired" }
        if (isGuestDns(address, port)) querySystemDns(flow, address.copyOf(), payload.copyOf())
        else socket.send(DatagramPacket(payload, payload.size, destination))
        udpSent.incrementAndGet()
        return payload.size
    }
    private fun querySystemDns(flow: Flow, address: ByteArray, payload: ByteArray) {
        require(payload.size in 12..65507) { "Invalid DNS query size" }
        check(dnsSlots.tryAcquire()) { "The guest reached its pending DNS query limit" }
        val signal = CancellationSignal()
        val finished = AtomicBoolean(false)
        val work = object : Closeable {
            override fun close() {
                if (finished.compareAndSet(false, true)) {
                    signal.cancel()
                    flow.queries.remove(this)
                    dnsSlots.release()
                }
            }
        }
        flow.queries.add(work)
        if (flow.closed.get() || !allowed()) { work.close(); return }
        try {
            val deadline = deadlines.schedule({ work.close() }, 20, TimeUnit.SECONDS)
            // This broker grants only the explicit DNS capability. The isolated caller
            // has no network permission; the app owns this platform resolver request.
            val identity = Binder.clearCallingIdentity()
            try {
                resolver.rawQuery(null, payload, DnsResolver.FLAG_EMPTY, threads, signal,
                    object : DnsResolver.Callback<ByteArray> {
                        override fun onAnswer(answer: ByteArray, rcode: Int) {
                            dnsAnswers.incrementAndGet()
                            try {
                                if (!finished.get() && !flow.closed.get() && allowed() && answer.size <= 65507) {
                                    val frame = datagramEnvelope(address, 53, answer)
                                    synchronized(flow.writeLock) {
                                        if (!finished.get() && !flow.closed.get()) {
                                            writeAll(flow.channel.fileDescriptor, frame, frame.size)
                                            udpReceived.incrementAndGet()
                                        }
                                    }
                                }
                            } catch (_: Exception) { streamErrors.incrementAndGet(); flow.close() }
                            finally { deadline.cancel(false); work.close() }
                        }
                        override fun onError(error: DnsResolver.DnsException) {
                            dnsErrors.incrementAndGet()
                            deadline.cancel(false); work.close()
                        }
                    })
                dnsRequested.incrementAndGet()
            } finally { Binder.restoreCallingIdentity(identity) }
        } catch (error: Exception) {
            work.close()
            throw IllegalStateException("Android could not resolve the guest DNS query", error)
        }
    }
    override fun closeFlow(handle: Int) { flows.remove(handle)?.close() }
    override fun localPort(handle: Int): Int = when (val socket = flows[handle]?.socket) {
        is Socket -> socket.localPort
        is DatagramSocket -> socket.localPort
        else -> error("The guest network capability has expired")
    }
    override fun close() {
        val current = synchronized(this) {
            closed = true
            flows.values.toList().also { flows.clear() }
        }
        current.forEach(Flow::close)
        deadlines.shutdownNow()
        threads.shutdownNow()
    }
    private fun writeAll(descriptor: FileDescriptor, bytes: ByteArray, size: Int) {
        var offset = 0
        while (offset < size) {
            check(allowed()) { "The owner has stopped guest networking" }
            val count = Os.write(descriptor, bytes, offset, size - offset)
            check(count > 0) { "The guest network stream closed" }
            offset += count
        }
    }
}
