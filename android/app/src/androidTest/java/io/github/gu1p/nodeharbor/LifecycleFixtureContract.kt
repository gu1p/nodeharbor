package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class LifecycleFixtureContract {
    @Test fun returningToNormalDoesNotReuseCloudInitCompletionFromAnEarlierTestSeed() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("owned-vm-contract-fixture-${UUID.randomUUID()}")
        val owner = UUID.randomUUID().toString()
        val guest = OwnedGuest(context, root)
        try {
            val receipt = guest.claim(owner, 15, "test")
            guest.file("root.img").writeText("preserved disk and enrollment marker")
            val identities = listOf(null, "unresponsive", null).map { mode ->
                guest.refreshSeed(receipt, stopped = true)
                installLifecycleMarker(guest, owner, mode)
                val image = guest.file("seed.iso").readBytes().toString(Charsets.UTF_8)
                checkNotNull(Regex("instance-id: ([^\\n]+)").find(image)).groupValues[1]
            }
            assertEquals("Each test configuration application must run despite a previous instance cache", 3, identities.toSet().size)
            assertEquals(receipt, guest.receipt(owner))
            assertEquals("preserved disk and enrollment marker", guest.file("root.img").readText())
        } finally { root.deleteRecursively() }
    }
}
