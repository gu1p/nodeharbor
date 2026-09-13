package io.github.gu1p.nodeharbor

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class VmSessionContract {
    @Test fun incompleteStorageCannotStartAContributingVm() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = context.cacheDir.resolve("vm-session-${UUID.randomUUID()}")
        val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
        try {
            val guest = OwnedGuest(context, root)
            guest.claim(owner, 15, "ubuntu-24.04-20260911")
            assertThrows(IllegalStateException::class.java) {
                VmSession.start(context, owner, PhonePolicy(), guest, { true }, {})
            }
            assertFalse(guest.receipt(owner)!!.complete)
        } finally { root.deleteRecursively() }
    }
}
