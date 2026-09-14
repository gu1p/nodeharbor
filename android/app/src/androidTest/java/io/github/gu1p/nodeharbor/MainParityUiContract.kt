package io.github.gu1p.nodeharbor

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performScrollTo
import org.junit.Rule
import org.junit.Test

class MainParityUiContract {
    @get:Rule val compose = createComposeRule()

    @Test fun workloadAndSystemHealthHaveSeparateAccessibleSections() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithText("Running workloads").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("System components").performScrollTo().assertIsDisplayed()
    }

    @Test fun appUpdatesExposesAnOwnerControlledNativePanel() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithText("App updates").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Check for updates").performScrollTo().assertIsDisplayed()
    }

    @Test fun storageLocationsExplainsUnavailableOrSelectablePhoneStorage() {
        compose.setContent { NodeHarborApp(UiState(), {}, { _, _ -> }) }
        compose.onNodeWithText("Storage locations").performScrollTo().assertIsDisplayed()
    }
}
