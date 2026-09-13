package io.github.gu1p.nodeharbor

import org.junit.Assert.*
import org.junit.Test

class PhonePolicyTest {
    @Test fun android17RequiresAnExplicitLocalNetworkGrantBeforeWorkerNetworking() {
        val phone = PhoneObservation(37, true, 4096, 50, true, 100, false, true, 0, false, true, localNetworkAllowed = false)
        assertFalse(phoneDecision(PhonePolicy(), phone, 2048).allowed)
        assertTrue(phoneDecision(PhonePolicy(), phone.copy(localNetworkAllowed = true), 2048).allowed)
    }
    private val capable = PhoneObservation(
        sdk = 33, arm64 = true, availableMemoryMib = 4096, freeDiskGib = 50,
        charging = true, batteryPercent = 80, metered = false, networkAvailable = true,
        thermal = 0, screenOn = true, runtimeReady = true,
    )

    @Test fun continuousPresetRequiresAnExplicitSharingAction() {
        val policy = PhonePolicy().continuous()
        assertTrue(policy.background && policy.startOnOpen && policy.startAfterBoot && policy.preventSleep)
        assertFalse(policy.allowMetered)
        assertFalse(policy.screenOffOnly)
    }

    @Test fun foregroundAndBackgroundUseTheSameOwnerConstraints() {
        val policy = PhonePolicy().continuous()
        assertTrue(phoneDecision(policy, capable, 2048).allowed)
        assertFalse(phoneDecision(policy, capable.copy(charging = false), 2048).allowed)
        assertFalse(phoneDecision(policy, capable.copy(metered = true), 2048).allowed)
        assertFalse(phoneDecision(policy, capable.copy(thermal = 3), 2048).allowed)
        assertFalse(phoneDecision(policy, capable.copy(runtimeReady = false), 2048).allowed)
    }

    @Test fun lowMemoryDoesNotStartTheVmOrEvictOtherApps() {
        val result = phoneDecision(PhonePolicy(), capable.copy(availableMemoryMib = 2300), 2048)
        assertFalse(result.allowed)
        assertTrue(result.reason.contains("memory", ignoreCase = true))
    }

    @Test fun screenOffMeansScreenOffRatherThanInventedDesktopIdleTime() {
        val policy = PhonePolicy(screenOffOnly = true)
        assertFalse(phoneDecision(policy, capable, 2048).allowed)
        assertTrue(phoneDecision(policy, capable.copy(screenOn = false), 2048).allowed)
    }

    @Test fun unknownPhoneInformationCannotEnableWork() {
        assertFalse(phoneDecision(PhonePolicy(), capable.copy(charging = null), 2048).allowed)
        assertFalse(phoneDecision(PhonePolicy(), capable.copy(thermal = null), 2048).allowed)
        assertFalse(phoneDecision(PhonePolicy(), capable.copy(sdk = 32), 2048).allowed)
    }

    @Test fun explicitStopsAndUpdatesCannotAutoRestart() {
        val policy = PhonePolicy().continuous()
        assertFalse(shouldAutoStart(policy, StartCause.Boot, false, false, false))
        assertFalse(shouldAutoStart(policy, StartCause.Open, true, true, false))
        assertFalse(shouldAutoStart(policy, StartCause.Open, true, false, true))
        assertTrue(shouldAutoStart(policy, StartCause.Boot, true, false, false))
    }
}
