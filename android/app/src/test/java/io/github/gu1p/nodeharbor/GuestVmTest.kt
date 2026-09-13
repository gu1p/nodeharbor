package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class GuestVmTest {
    @Test fun managedGuestBootDoesNotStartUnusedDesktopFirmwareOrSnapServices() {
        val args = guestArguments(PhonePolicy(), listOf(10, 11, 12, 13, 17), 14, 15, 16)
        val boot = args[args.indexOf("-append") + 1]
        assertTrue(boot.contains("systemd.mask=fwupd.service"))
        assertTrue(boot.contains("systemd.mask=snapd.service"))
        assertFalse(boot.contains("systemd.mask=nodeharbor"))
        assertFalse(boot.contains("systemd.mask=systemd-networkd"))
    }
    @Test fun ownedGuestBootsFromExplicitDescriptorsWithNoHostExports() {
        val args = guestArguments(PhonePolicy(), listOf(10, 11, 12, 13, 17), 14, 15, 16)
        val text = args.joinToString(" ")
        assertTrue(text.contains("root=/dev/vda rw"))
        assertTrue(text.contains("readonly=on"))
        assertTrue(text.contains("fd=17,set=0"))
        assertTrue(text.contains("tcg,thread=multi"))
        assertTrue(text.contains("name=io.github.gu1p.nodeharbor.control"))
        assertFalse(text.contains("/sdcard"))
        assertFalse(text.contains("/dev/kvm"))
        assertFalse(text.contains("hostfwd"))
        assertFalse(text.contains("virtfs"))
    }
    @Test fun missingDuplicateAndInvalidCapabilitiesFailBeforeStartingQemu() {
        for (files in listOf(listOf(10, 11, 12, 13), listOf(10, 10, 12, 13, 17), listOf(0, 11, 12, 13, 17))) {
            assertThrows(IllegalArgumentException::class.java) { guestArguments(PhonePolicy(), files, 14, 15, 16) }
        }
        assertThrows(IllegalArgumentException::class.java) { guestArguments(PhonePolicy(memoryMib = 0), listOf(10, 11, 12, 13, 17), 14, 15, 16) }
    }
    @Test fun guestHasAnEntropyDeviceForKeyGenerationWithoutPhoneFileExports() {
        val args = guestArguments(PhonePolicy(), listOf(10, 11, 12, 13, 17), 14, 15, 16)
        assertTrue(args.contains("rng-builtin,id=entropy"))
        assertTrue(args.contains("virtio-rng-pci,rng=entropy"))
    }
}
