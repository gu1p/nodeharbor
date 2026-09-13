package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

class WakeLockContract {
    @Test fun processingWakeLeaseIsExplicitAndReleasedWhenWorkStops() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        WorkerWakeLock(context).use { wake ->
            assertFalse(wake.held)
            wake.update(false)
            assertFalse(wake.held)
            wake.update(true)
            assertTrue(wake.held)
            wake.update(false)
            assertFalse(wake.held)
        }
    }
}
