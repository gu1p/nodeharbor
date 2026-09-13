package io.github.gu1p.nodeharbor

import android.content.Context
import android.os.PowerManager
import java.io.Closeable

/** Renew only while the owner allows processing or its bounded drain is active. */
class WorkerWakeLock(context: Context) : Closeable {
    private val wake = context.getSystemService(PowerManager::class.java)
        .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "NodeHarbor:worker").apply { setReferenceCounted(false) }
    private var closed = false
    val held: Boolean get() = wake.isHeld
    @Synchronized fun update(needed: Boolean) {
        if (closed) return
        if (needed) wake.acquire(120_000)
        else if (wake.isHeld) wake.release()
    }
    @Synchronized override fun close() {
        closed = true
        if (wake.isHeld) wake.release()
    }
}
