package io.github.gu1p.nodeharbor

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsOff
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.graphics.asAndroidBitmap
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class OwnerUiContract {
    @get:Rule val compose = createComposeRule()

    @Test fun unconfirmedShutdownIsAccessibleAndBlocksRestartAndReplacement() {
        compose.setContent { NodeHarborApp(UiState(enrolled = true, workerInstalled = true,
            state = "shutdown-unconfirmed", reason = "Shutdown unconfirmed. Worker files have been preserved."), {}, { _, _ -> }) }
        compose.onNodeWithText("Shutdown unconfirmed. Worker files have been preserved.").assertIsDisplayed()
        compose.onNodeWithText("Enable sharing").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithText("Replace worker").performScrollTo().assertIsNotEnabled()
    }

    @Test fun stoppingIsAccessibleAndCannotStartAnotherWorker() {
        compose.setContent { NodeHarborApp(UiState(enrolled = true, workerInstalled = true,
            state = "stopping", reason = "Stopping. Waiting for guest poweroff and Android process death."), {}, { _, _ -> }) }
        compose.onNodeWithText("Stopping.", substring = true).assertIsDisplayed()
        compose.onNodeWithText("Enable sharing").performScrollTo().assertIsNotEnabled()
    }

    @Test fun forcedStopHasAnAccessibleOutcome() {
        compose.setContent { NodeHarborApp(UiState(state = "forced-stop", reason = "Worker force-stopped."), {}, { _, _ -> }) }
        compose.onNodeWithText("Worker force-stopped.").assertIsDisplayed()
    }

    @Test fun theOwnerCanInspectTheBuildIdentityAndRuntimeSourceOffer() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithContentDescription("Connection").performClick()
        compose.onNodeWithText("Source and licenses").performScrollTo().performClick()
        compose.onNodeWithText("Source commit: ${BuildConfig.SOURCE_COMMIT}").assertIsDisplayed()
        compose.onNodeWithText("QEMU 11.1.1", substring = true).assertIsDisplayed()
        compose.onNodeWithText("Get corresponding sources").assertIsDisplayed()
    }

    @Test fun invalidNumbersCannotSilentlySaveAnOlderLimit() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithContentDescription("Sharing rules").performClick()
        compose.onNodeWithText("CPU allowance").performScrollTo().performTextReplacement("")
        compose.onNodeWithText("Save sharing rules").performScrollTo().assertIsNotEnabled()
        val error = compose.onNodeWithText("CPU allowance: enter a whole number").performScrollTo()
        try { error.assertIsDisplayed() }
        catch (failure: AssertionError) {
            val context = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().targetContext
            context.filesDir.resolve("owner-ui-contract.png").outputStream().use { output ->
                compose.onRoot().captureToImage().asAndroidBitmap().compress(android.graphics.Bitmap.CompressFormat.PNG, 100, output)
            }
            println("Error bounds: ${error.fetchSemanticsNode().boundsInRoot}")
            println("Root bounds: ${compose.onRoot().fetchSemanticsNode().boundsInRoot}")
            throw failure
        }
    }

    @Test fun aDailyScheduleCanBeSetUsingLabeledNativeControls() {
        var saved: PhonePolicy? = null
        compose.setContent { NodeHarborApp(UiState(), { if (it is UiAction.SavePolicy) saved = it.policy }, { _, _ -> }) }
        compose.onNodeWithContentDescription("Sharing rules").performClick()
        compose.onNodeWithContentDescription("Use a schedule").performScrollTo().performClick()
        compose.onNodeWithText("Start time (HH:mm)").performScrollTo().performTextReplacement("22:00")
        compose.onNodeWithText("End time (HH:mm)").performScrollTo().performTextReplacement("06:00")
        compose.onNodeWithText("Save sharing rules").performScrollTo().performClick()
        assertEquals(true, saved?.scheduleEnabled)
        assertEquals(ScheduleWindow((0..6).toList(), 1320, 360), saved?.schedule?.single())
    }

    @Test fun ownerCanStopEvenWhilePreparationIsBusy() {
        var requested: UiAction? = null
        compose.setContent { NodeHarborApp(UiState(enrolled = true, busy = true), { requested = it }, { _, _ -> }) }
        compose.onNodeWithText("Pause").performScrollTo().assertIsEnabled()
        compose.onNodeWithText("Stop").performScrollTo().assertIsEnabled().performClick()
        compose.onNodeWithText("Stop worker").performClick()
        assertEquals(UiAction.Stop, requested)
    }

    @Test fun installationIsOffAndNavigationUsesAccessibleNames() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithText("Sharing is switched off").assertIsDisplayed()
        listOf("Your phone", "Sharing rules", "Fleet", "Connection").forEach {
            compose.onNodeWithContentDescription(it).assertIsDisplayed()
        }
    }

    @Test fun continuousSetupDoesNotSilentlyGrantSystemPermissions() {
        var requested: UiAction? = null
        compose.setContent { NodeHarborApp(UiState(), { requested = it }, { _, _ -> }) }
        compose.onNodeWithContentDescription("Sharing rules").performClick()
        compose.onNodeWithText("Continuous sharing").performScrollTo().performClick()
        assertEquals(UiAction.ContinuousSetup, requested)
        compose.onNodeWithText("Battery optimization: not exempt").performScrollTo().assertIsDisplayed()
    }

    @Test fun batteryAndMeteredSharingAreExplicitlyOff() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithContentDescription("Sharing rules").performClick()
        compose.onNodeWithContentDescription("Run while unplugged").performScrollTo().assertIsOff()
        compose.onNodeWithContentDescription("Use metered connections").performScrollTo().assertIsOff()
    }

    @Test fun unavailableRuntimeExplainsWhyPreparationCannotStart() {
        compose.setContent {
            NodeHarborApp(UiState(runtimeReason = "The packaged VM runtime is unavailable"), {}, { _, _ -> })
        }
        compose.onNodeWithText("The packaged VM runtime is unavailable").assertIsDisplayed()
    }
}
