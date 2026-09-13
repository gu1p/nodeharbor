package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

class PrivateStoreContract {
    @Test fun persistedCredentialsAreEncryptedAndSettingsCorruptionIsPreserved() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = context.filesDir.resolve("store-contract").apply { mkdirs() }
        try {
            val store = PrivateStore(directory, "nodeharbor-test-credentials")
            assertFalse(store.load().policy.enabled)
            val secret = "test-only-token-that-is-not-a-credential"
            store.saveToken(secret)
            assertEquals(secret, store.token())
            assertFalse(directory.resolve("credentials.bin").readBytes().toString(Charsets.UTF_8).contains(secret))
            store.update { it.copy(policy = it.policy.continuous()) }
            assertTrue(PrivateStore(directory, "nodeharbor-test-credentials").load().policy.background)
            directory.resolve("settings.json").writeText("damaged original")
            assertThrows(IllegalArgumentException::class.java) { store.load() }
            assertEquals("damaged original", directory.resolve("settings.json").readText())
        } finally {
            directory.deleteRecursively()
        }
    }
}
