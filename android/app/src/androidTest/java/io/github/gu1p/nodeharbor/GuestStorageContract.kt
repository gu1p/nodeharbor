package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class GuestStorageContract {
    private val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
    @Test fun reservingTheOwnedDiskDoesNotConsumeTheContributionBudgetTwice() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("allocation-${UUID.randomUUID()}")
        val guest = OwnedGuest(context, root)
        try {
            guest.claim(owner, 15, "ubuntu-24.04-20260911")
            assertEquals(0L, guest.allocatedBytes(owner))
            val staged = guest.file("root.img.preparing")
            java.io.RandomAccessFile(staged, "rw").use {
                it.setLength(15L * 1024 * 1024 * 1024)
                android.system.Os.posix_fallocate(it.fd, 0, 8L * 1024 * 1024)
                it.fd.sync()
            }
            val reserved = guest.allocatedBytes(owner)
            assertTrue("Count actual allocated blocks, including preparation", reserved >= 8L * 1024 * 1024)
            assertTrue("Sparse logical length is not free storage", reserved < 16L * 1024 * 1024)
            java.nio.file.Files.move(staged.toPath(), guest.file("root.img").toPath())
            assertEquals(reserved, guest.allocatedBytes(owner))
        } finally { if (guest.receipt(owner) != null) guest.remove(owner, true, true) }
    }
    @Test fun unrecognizedFilesArePreservedInsteadOfClaimedOrDeleted() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("unowned-${UUID.randomUUID()}").apply { mkdir() }
        try {
            val disk = root.resolve("root.img").apply { writeText("unrecognized disk") }
            val guest = OwnedGuest(context, root)
            assertThrows(IllegalStateException::class.java) { guest.claim(owner, 15, "ubuntu-24.04-20260911") }
            assertEquals("unrecognized disk", disk.readText())
        } finally { root.deleteRecursively() }
    }
    @Test fun ownedDiskRemovalRequiresBothShutdownAndFleetReset() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("owned-${UUID.randomUUID()}")
        try {
            val guest = OwnedGuest(context, root)
            guest.claim(owner, 15, "ubuntu-24.04-20260911")
            root.resolve("root.img").writeText("owned disk")
            assertThrows(IllegalStateException::class.java) { guest.remove(owner, stopped = true, reset = false) }
            assertTrue(root.resolve("root.img").isFile)
            guest.remove(owner, stopped = true, reset = true)
            assertFalse(root.exists())
        } finally { root.deleteRecursively() }
    }
}
