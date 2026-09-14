package io.github.gu1p.nodeharbor

import android.app.Service
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.IBinder
import android.os.Process
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Android assigns a separate isolated UID; no app permissions or private paths. */
class SandboxService : Service() {
    private val started = AtomicBoolean(false)
    override fun onDestroy() {
        super.onDestroy()
        // Android invokes this when the final owner binding is released. There
        // must be no emulator threads left after the service lifetime ends.
        Process.killProcess(Process.myPid())
    }
    override fun onBind(intent: Intent): IBinder = object : ISandbox.Stub() {
        override fun boot(disk: ParcelFileDescriptor, kernel: ParcelFileDescriptor, initrd: ParcelFileDescriptor,
                          seed: ParcelFileDescriptor, console: ParcelFileDescriptor, control: ParcelFileDescriptor,
                          diskReader: ParcelFileDescriptor, broker: ISocketBroker, cpus: Int, memoryMib: Int) {
            bootPool(disk, kernel, initrd, seed, console, control, diskReader, emptyArray(), emptyArray(), broker, cpus, memoryMib)
        }
        override fun bootPool(disk: ParcelFileDescriptor, kernel: ParcelFileDescriptor, initrd: ParcelFileDescriptor,
                          seed: ParcelFileDescriptor, console: ParcelFileDescriptor, control: ParcelFileDescriptor,
                          diskReader: ParcelFileDescriptor, storage: Array<ParcelFileDescriptor>, readers: Array<ParcelFileDescriptor>,
                          broker: ISocketBroker, cpus: Int, memoryMib: Int) {
            require(storage.size == readers.size && storage.size <= 16) { "Invalid owned storage descriptors" }
            val files = listOf(disk, kernel, initrd, seed, diskReader) + storage + readers
            val ownedDisks = listOf(disk) + storage
            val readDisks = listOf(diskReader) + readers
            val identities = ownedDisks.mapIndexed { index, file ->
                val identity = Os.fstat(file.fileDescriptor)
                val reader = Os.fstat(readDisks[index].fileDescriptor)
                require(OsConstants.S_ISREG(identity.st_mode) && identity.st_size > 0 &&
                    identity.st_dev == reader.st_dev && identity.st_ino == reader.st_ino &&
                    Os.fcntlInt(file.fileDescriptor, OsConstants.F_GETFL, 0) and OsConstants.O_ACCMODE == OsConstants.O_RDWR &&
                    Os.fcntlInt(readDisks[index].fileDescriptor, OsConstants.F_GETFL, 0) and OsConstants.O_ACCMODE == OsConstants.O_RDONLY) {
                    "Each owned disk needs matching writable and read-only descriptors"
                }
                identity.st_dev to identity.st_ino
            }
            require(identities.distinct().size == identities.size) { "A storage member cannot be attached more than once" }
            check(started.compareAndSet(false, true)) { "The isolated runtime is already in use" }
            val network = ParcelFileDescriptor.createSocketPair()
            val args = guestArguments(PhonePolicy(cpus = cpus, memoryMib = memoryMib), files.take(5).map { it.fd }, console.fd, control.fd, network[0].fd,
                storage.zip(readers).map { it.first.fd to it.second.fd })
            for (file in files) {
                val flags = Os.fcntlInt(file.fileDescriptor, OsConstants.F_GETFD, 0)
                Os.fcntlInt(file.fileDescriptor, OsConstants.F_SETFD, flags and OsConstants.FD_CLOEXEC.inv())
            }
            files.forEach { it.detachFd() }
            val consoleFd = console.detachFd()
            control.detachFd(); network[0].detachFd()
            val packetFd = network[1].detachFd()
            Thread({ NativeNetwork.run(packetFd, byteArrayOf(10, 0, 2, 3), SocketBridge(broker)) }, "nodeharbor-packets").start()
            Thread({
                try { NativeRuntime.redirectOutput(consoleFd); NativeRuntime.run(args) }
                finally { Process.killProcess(Process.myPid()) }
            }, "nodeharbor-guest").start()
        }
        override fun guestNetworkProbe(broker: ISocketBroker, dns: ByteArray): Boolean = io.github.gu1p.nodeharbor.guestNetworkProbe(broker, dns)
        override fun networkProbe(broker: ISocketBroker, address: ByteArray): String {
            try {
                broker.openTcp(7, address, 80).use { file ->
                    val request = "GET / HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n\r\n".toByteArray()
                    var written = 0
                    while (written < request.size) written += Os.write(file.fileDescriptor, request, written, request.size - written)
                    val response = ByteArray(1024)
                    val count = Os.read(file.fileDescriptor, response, 0, response.size)
                    return if (count > 0) response.copyOf(count).toString(Charsets.UTF_8) else ""
                }
            } finally { broker.closeFlow(7) }
        }
        override fun bootProbe(kernel: ParcelFileDescriptor, initrd: ParcelFileDescriptor, console: ParcelFileDescriptor) {
            check(started.compareAndSet(false, true)) { "The isolated runtime is already in use" }
            // QEMU distinguishes explicitly inherited files from its internal
            // descriptors using CLOEXEC. Binder supplies these two owned files
            // instead of exec; preserve every other descriptor's flags.
            for (file in listOf(kernel, initrd)) {
                val flags = Os.fcntlInt(file.fileDescriptor, OsConstants.F_GETFD, 0)
                Os.fcntlInt(file.fileDescriptor, OsConstants.F_SETFD, flags and OsConstants.FD_CLOEXEC.inv())
            }
            val kernelFd = kernel.detachFd()
            val initrdFd = initrd.detachFd()
            val consoleFd = console.detachFd()
            Thread({
                try {
                    NativeRuntime.redirectOutput(consoleFd)
                    NativeRuntime.run(arrayOf("nodeharbor-qemu", "-machine", "virt,gic-version=3", "-cpu", "cortex-a57",
                        "-accel", "tcg,thread=multi", "-smp", "2", "-m", "512", "-nodefaults", "-no-user-config",
                        "-display", "none", "-monitor", "none", "-no-reboot",
                        "-add-fd", "fd=$kernelFd,set=0", "-add-fd", "fd=$initrdFd,set=1",
                        "-kernel", "/dev/fdset/0", "-initrd", "/dev/fdset/1", "-append", "console=ttyAMA0 panic=-1",
                        "-chardev", "socket,id=console,fd=$consoleFd", "-serial", "chardev:console"))
                } finally { Process.killProcess(Process.myPid()) }
            }, "nodeharbor-guest").start()
        }
        override fun stop() { Process.killProcess(Process.myPid()) }
        override fun inspect(privatePath: String): Bundle {
            val canRead = runCatching { File(privatePath).inputStream().use { it.read() }; true }.getOrDefault(false)
            val network = runCatching {
                Os.close(Os.socket(OsConstants.AF_INET, OsConstants.SOCK_STREAM, 0))
                true
            }.getOrDefault(false)
            return Bundle().apply {
                putInt("uid", Process.myUid())
                putBoolean("canReadPrivateFile", canRead)
                putBoolean("canOpenNetwork", network)
                putString("runtimeVersion", runCatching { NativeRuntime.version() }.getOrDefault(""))
            }
        }
    }
}

data class SandboxInspection(val uid: Int, val canReadPrivateFile: Boolean, val canOpenNetwork: Boolean, val runtimeVersion: String)

class SandboxClient(private val context: Context) {
    fun bootGuestProbe(disk: File, kernel: File, initrd: File, seed: File): String = withSandbox { sandbox ->
        val console = ParcelFileDescriptor.createSocketPair()
        val control = ParcelFileDescriptor.createSocketPair()
        val reader = Executors.newSingleThreadExecutor()
        NetworkBroker(context) { true }.use { broker ->
            val files = listOf(disk, kernel, initrd, seed, disk).mapIndexed { index, file ->
                ParcelFileDescriptor.open(file, if (index == 0) ParcelFileDescriptor.MODE_READ_WRITE else ParcelFileDescriptor.MODE_READ_ONLY)
            }
            try {
                sandbox.boot(files[0], files[1], files[2], files[3], console[1], control[1], files[4], broker, 2, 2048)
                files.forEach { it.close() }; console[1].close(); control[1].close()
                reader.submit<String> {
                    ParcelFileDescriptor.AutoCloseInputStream(console[0]).use { input ->
                        val collected = java.io.ByteArrayOutputStream()
                        // Test-only guest console: retained privately even if the
                        // boot times out, so a failure does not discard evidence.
                        disk.parentFile!!.resolve("console.log").outputStream().use { log ->
                            val buffer = ByteArray(8192)
                            while (true) {
                                val count = input.read(buffer)
                                if (count < 0) break
                                check(collected.size() + count <= 4 * 1024 * 1024) { "The guest exceeded its console limit" }
                                collected.write(buffer, 0, count); log.write(buffer, 0, count)
                            }
                        }
                        collected.toByteArray().toString(Charsets.UTF_8)
                    }
                }.get(600, TimeUnit.SECONDS)
            } finally {
                runCatching { sandbox.stop() }
                runCatching { Os.shutdown(console[0].fileDescriptor, OsConstants.SHUT_RDWR) }
                (files + console + control).forEach { runCatching { it.close() } }
                reader.shutdownNow()
            }
        }
    }
    fun guestNetworkProbe(dns: ByteArray): Boolean = withSandbox { sandbox ->
        NetworkBroker(context) { true }.use { broker ->
            val executor = Executors.newSingleThreadExecutor { task -> Thread(task, "nodeharbor-guest-network-probe").apply { isDaemon = true } }
            try { executor.submit<Boolean> { sandbox.guestNetworkProbe(broker, dns) }.get(30, TimeUnit.SECONDS) }
            catch (error: Exception) { throw IllegalStateException("The isolated guest network failed (${broker.diagnosticCounts()})", error) }
            finally { runCatching { sandbox.stop() }; executor.shutdownNow() }
        }
    }
    fun networkProbe(address: ByteArray): String = withSandbox { sandbox ->
        NetworkBroker(context) { true }.use { broker ->
            val executor = Executors.newSingleThreadExecutor { task -> Thread(task, "nodeharbor-network-probe").apply { isDaemon = true } }
            try { executor.submit<String> { sandbox.networkProbe(broker, address) }.get(30, TimeUnit.SECONDS) }
            finally { runCatching { sandbox.stop() }; executor.shutdownNow() }
        }
    }
    fun inspect(path: String): SandboxInspection = withSandbox { sandbox ->
        val result = sandbox.inspect(path)
        SandboxInspection(result.getInt("uid"), result.getBoolean("canReadPrivateFile"), result.getBoolean("canOpenNetwork"), result.getString("runtimeVersion", ""))
    }
    fun bootProbe(kernel: File, initrd: File): String = withSandbox { sandbox ->
        val pair = ParcelFileDescriptor.createSocketPair()
        val reader = Executors.newSingleThreadExecutor { task -> Thread(task, "nodeharbor-console").apply { isDaemon = true } }
        try {
            ParcelFileDescriptor.open(kernel, ParcelFileDescriptor.MODE_READ_ONLY).use { kernelFd ->
                ParcelFileDescriptor.open(initrd, ParcelFileDescriptor.MODE_READ_ONLY).use { initrdFd ->
                    pair[1].use { console -> sandbox.bootProbe(kernelFd, initrdFd, console) }
                }
            }
            reader.submit<String> {
                ParcelFileDescriptor.AutoCloseInputStream(pair[0]).use { input ->
                    val bytes = input.readNBytes(4 * 1024 * 1024 + 1)
                    check(bytes.size <= 4 * 1024 * 1024) { "The guest exceeded its console limit" }
                    bytes.toString(Charsets.UTF_8)
                }
            }.get(180, TimeUnit.SECONDS)
        } finally {
            runCatching { sandbox.stop() }
            runCatching { Os.shutdown(pair[0].fileDescriptor, OsConstants.SHUT_RDWR) }
            pair.forEach { runCatching { it.close() } }
            reader.shutdownNow()
        }
    }
    private fun <T> withSandbox(operation: (ISandbox) -> T): T {
        val binding = SandboxBinding(context)
        val sandbox = binding.service
        val dead = CountDownLatch(1)
        sandbox.asBinder().linkToDeath({ dead.countDown() }, 0)
        try {
            return operation(sandbox)
        } finally {
            binding.close()
            check(dead.await(10, TimeUnit.SECONDS)) { "Android has not confirmed isolated test process shutdown" }
        }
    }
}
