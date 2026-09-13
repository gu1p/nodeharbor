package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class SettingsInputTest {
    @Test fun timesRequireACompleteValid24HourClock() {
        assertEquals(1320, clockMinute("22:00"))
        assertEquals(0, clockMinute("00:00"))
        for (text in listOf("", "6", "6:00", "24:00", "12:60", "01:2", " 12:00")) assertNull(clockMinute(text))
    }
}
