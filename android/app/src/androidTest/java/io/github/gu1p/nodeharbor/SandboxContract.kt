package io.github.gu1p.nodeharbor

import android.os.Process
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

class SandboxContract {
    @Test fun emulatorRunsWithoutAccessToAppCredentials() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val secret = context.filesDir.resolve("sandbox-test-secret")
        secret.writeText("test-only-sentinel")
        try {
            val result = SandboxClient(context).inspect(secret.absolutePath)
            assertNotEquals(Process.myUid(), result.uid)
            assertFalse(result.canReadPrivateFile)
            assertFalse(result.canOpenNetwork)
            assertEquals("11.1.1", result.runtimeVersion)
        } finally {
            secret.delete()
        }
    }
}
