package io.github.gu1p.nodeharbor

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onAllNodesWithText
import org.junit.Rule
import org.junit.Test
import org.junit.Assert.assertFalse
import androidx.test.platform.app.InstrumentationRegistry

class LaunchContract {
    @get:Rule val ui = createAndroidComposeRule<MainActivity>()
    @Test fun installingAndOpeningTheNativeAppDoesNotStartSharing() {
        ui.onAllNodesWithText("Your phone")[0].assertExists()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val saved = PrivateStore(context.noBackupFilesDir.resolve("nodeharbor")).load()
        assertFalse("Installing or updating the app must leave sharing off", saved.policy.enabled)
        if (saved.deviceId.isEmpty()) ui.onNodeWithText("Connect to your fleet").assertIsDisplayed()
    }
}
