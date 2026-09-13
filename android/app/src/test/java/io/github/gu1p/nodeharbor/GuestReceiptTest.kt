package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class GuestReceiptTest {
    private val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
    private val generation = "b5f687c2-f923-4ef8-ac0f-dfbdbd76f327"
    @Test fun diskReceiptBindsTheOwnedFilesToEnrollmentAndImage() {
        val receipt = GuestReceipt(owner, generation, 15, "ubuntu-24.04-20260911", true)
        assertEquals(receipt, GuestReceipt.decode(receipt.encode(), owner))
        assertThrows(IllegalArgumentException::class.java) { GuestReceipt.decode(receipt.encode(), generation) }
        assertThrows(IllegalArgumentException::class.java) { GuestReceipt.decode("{}", owner) }
    }
    @Test fun ownedFilesCannotBeRemovedBeforeShutdownAndFleetResetAreConfirmed() {
        val receipt = GuestReceipt(owner, generation, 15, "ubuntu-24.04-20260911", true)
        assertTrue(canRemoveGuest(receipt, owner, stopped = true, reset = true))
        assertFalse(canRemoveGuest(receipt, owner, stopped = false, reset = true))
        assertFalse(canRemoveGuest(receipt, owner, stopped = true, reset = false))
        assertFalse(canRemoveGuest(receipt, generation, stopped = true, reset = true))
    }
}
