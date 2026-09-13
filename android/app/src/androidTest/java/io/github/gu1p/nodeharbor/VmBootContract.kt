package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertTrue
import org.junit.Test

class VmBootContract {
    @Test fun packagedTcgBootsLinuxUsingOnlyPassedDescriptors() {
        val test = InstrumentationRegistry.getInstrumentation()
        val context = test.targetContext
        val directory = context.cacheDir.resolve("boot-contract").apply { mkdirs() }
        try {
            for (name in listOf("kernel", "initrd")) test.context.assets.open("boot-probe/$name").use { input ->
                directory.resolve(name).outputStream().use { output -> input.copyTo(output) }
            }
            val output = SandboxClient(context).bootProbe(directory.resolve("kernel"), directory.resolve("initrd"))
            assertTrue("The isolated guest must execute its own Linux init process: ${output.takeLast(2000)}",
                output.contains("NODEHARBOR_ISOLATED_BOOT_OK"))
        } finally { directory.deleteRecursively() }
    }
}
