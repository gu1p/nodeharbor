package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class SandboxLifetimeContract {
    @Test fun bindingLossDoesNotSubstituteForTheOriginalBindersDeath() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        lateinit var connection: android.content.ServiceConnection
        val wrapper = object : android.content.ContextWrapper(context) {
            override fun bindIsolatedService(intent: android.content.Intent, flags: Int, instance: String,
                                             executor: java.util.concurrent.Executor, callback: android.content.ServiceConnection): Boolean {
                connection = callback
                return super.bindIsolatedService(intent, flags, instance, executor, callback)
            }
            override fun unbindService(callback: android.content.ServiceConnection) { /* Hold the real test binding. */ }
        }
        val death = CountDownLatch(1)
        val binding = SandboxBinding(wrapper, processDied = { death.countDown() })
        val original = binding.service
        try {
            connection.onBindingDied(android.content.ComponentName(context, SandboxService::class.java))
            assertTrue(original.asBinder().isBinderAlive)
            assertEquals("Lost binding is not process death", 1L, death.count)
            runCatching { original.stop() }
            assertTrue("Keep observing the original Binder after connection cleanup", death.await(5, TimeUnit.SECONDS))
        } finally { binding.close(); context.unbindService(connection) }
    }

    @Test fun separateOwnerSessionsUseSeparateIsolatedProcesses() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        SandboxBinding(context).use { first ->
            SandboxBinding(context).use { second ->
                assertNotEquals("A new owner session must not attach to a previous worker process",
                    first.service.inspect(context.filesDir.absolutePath).getInt("uid"),
                    second.service.inspect(context.filesDir.absolutePath).getInt("uid"))
            }
        }
    }
    @Test fun releasingTheOwnerBindingTerminatesItsIsolatedProcess() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val binding = SandboxBinding(context)
        val dead = CountDownLatch(1)
        binding.service.asBinder().linkToDeath({ dead.countDown() }, 0)
        binding.close()
        assertTrue("Android must confirm that the owned isolated process terminated", dead.await(10, TimeUnit.SECONDS))
    }
}
