package io.github.gu1p.nodeharbor

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.IBinder
import java.io.Closeable
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** One owner binding, including cleanup when Android terminates the guest. */
class SandboxBinding(private val context: Context, private val disconnected: () -> Unit = {},
                     private val processDied: () -> Unit = {}, private val autoConnect: Boolean = true) : Closeable {
    private val closed = AtomicBoolean(false)
    private val connected = CountDownLatch(1)
    @Volatile private var remote: ISandbox? = null
    @Volatile var original: ISandbox? = null
        private set
    private val death = IBinder.DeathRecipient { processDied() }
    val service: ISandbox get() = checkNotNull(remote) { "The isolated VM service is unavailable" }
    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            // Connection cleanup must never discard the Binder whose death we
            // need to observe. Android can report binding loss while it is alive.
            if (original == null) {
                original = ISandbox.Stub.asInterface(binder)
                try { binder.linkToDeath(death, 0) }
                catch (_: android.os.DeadObjectException) { processDied() }
            }
            if (!closed.get()) remote = original
            else Thread { runCatching { original?.stop() } }.start()
            connected.countDown()
        }
        override fun onServiceDisconnected(name: ComponentName) { remote = null; connected.countDown(); disconnected(); close() }
        override fun onBindingDied(name: ComponentName) { remote = null; connected.countDown(); disconnected(); close() }
        override fun onNullBinding(name: ComponentName) { connected.countDown() }
    }
    init { if (autoConnect) connect() }
    fun connect() {
        try {
            check(!closed.get()) { "The isolated VM binding is closed" }
            // A fresh Android service instance cannot race a previous worker's
            // asynchronous destruction or inherit its process state.
            val instance = "worker_" + java.util.UUID.randomUUID().toString().replace("-", "")
            check(context.bindIsolatedService(Intent(context, SandboxService::class.java), Context.BIND_AUTO_CREATE,
                instance, context.mainExecutor, connection)) {
                "Android could not start the isolated VM service"
            }
            if (closed.get()) runCatching { context.unbindService(connection) }
            check(connected.await(15, TimeUnit.SECONDS) && remote != null) { "The isolated VM service did not respond" }
        } catch (error: Exception) { close(); throw error }
    }
    override fun close() {
        if (closed.compareAndSet(false, true)) {
            connected.countDown()
            runCatching { context.unbindService(connection) }
        }
    }
}
