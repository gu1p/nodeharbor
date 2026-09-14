package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test

/** Run seed on the lower signed version, then verify after adb install -r. */
class SignedUpdateContract {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val directory get() = context.filesDir.resolve("signed-update-contract")
    private val marker get() = InstrumentationRegistry.getArguments().getString("marker")!!

    @Test fun seed() {
        directory.mkdirs()
        val store = PrivateStore(directory, "nodeharbor-signed-update-contract")
        store.saveToken(marker)
        store.update { it.copy(policy = it.policy.continuous()) }
        directory.resolve("workload-marker").writeText(marker)
    }

    @Test fun verify() {
        try {
            val expected = InstrumentationRegistry.getArguments().getString("versionCode")!!.toLong()
            val installed = context.packageManager.getPackageInfo(context.packageName, android.content.pm.PackageManager.PackageInfoFlags.of(0))
            assertEquals(expected, installed.longVersionCode)
            val store = PrivateStore(directory, "nodeharbor-signed-update-contract")
            assertEquals(marker, store.token())
            assertTrue(store.load().policy.background)
            assertEquals(marker, directory.resolve("workload-marker").readText())
        } finally {
            directory.deleteRecursively()
        }
    }
}
