package io.github.gu1p.nodeharbor

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test

class StorageUiContract {
    @get:Rule val compose = createComposeRule()

    @Test fun storageReviewExplainsTheExistingBoundedDrainBehavior() {
        val review = PhoneStorageReview(listOf(PhoneStorageDisk.new("internal", 15)), 1, emptyMap(), false)
        compose.setContent { NodeHarborApp(UiState(storage = StorageUiState(review = review)), {}, { _, _ -> }) }
        compose.onNodeWithText("Current jobs drain before storage maintenance and may be interrupted at your drain deadline. Enrollment and sharing preferences are preserved.")
            .performScrollTo().assertIsDisplayed()
    }

    @Test fun changingAllocationRequiresAnAccessibleReviewBeforeApplying() {
        val actions = mutableListOf<UiAction>()
        compose.setContent { NodeHarborApp(UiState(enrolled = true, storage = StorageUiState(
            locations = listOf(PhoneStorageLocation("internal", "Internal storage", "1", 80 * STORAGE_GIB, true)))), actions::add, { _, _ -> }) }
        compose.onNodeWithText("Storage locations").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Edit storage locations").performScrollTo().performClick()
        compose.onNodeWithText("Disk 1 allocation in GiB").performTextReplacement("30")
        compose.onNodeWithText("Review storage changes").assertIsDisplayed().performClick()
        assertEquals(30, (actions.single() as UiAction.ReviewStorage).disks.single().gib)
    }

    @Test fun missingStorageDoesNotOfferSilentFallbackOrClaimAnEmptyPool() {
        compose.setContent { NodeHarborApp(UiState(storage = StorageUiState(message = "Selected storage is unavailable. The whole worker is stopped.",
            missing = true)), {}, { _, _ -> }) }
        compose.onNodeWithText("Selected storage is unavailable. The whole worker is stopped.").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Automatically replace lost storage").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Delete all worker storage").performScrollTo().assertIsDisplayed()
    }
    @Test fun aReturnedDiskAddsReviewedCapacityWithoutAdoptingItsStaleContents() {
        val owner = "9511182e-9c48-4d20-a15b-1da8bb441386"
        val active = PhoneStorageDisk.new("internal", 15)
        val excluded = PhoneStorageDisk.new("primary", 20)
        val pool = PhoneStoragePool(owner, "00000000-0000-4000-8000-000000000001", 4, listOf(active))
        val actions = mutableListOf<UiAction>()
        compose.setContent { NodeHarborApp(UiState(enrolled = true, storage = StorageUiState(pool = pool, excluded = listOf(excluded),
            locations = listOf(PhoneStorageLocation("primary", "Returned storage", "2", 50 * STORAGE_GIB, true)))), actions::add, { _, _ -> }) }
        compose.onNodeWithText("Restore 20 GiB on Returned storage").performScrollTo().performClick()
        val disks = (actions.single() as UiAction.ReviewStorage).disks
        assertEquals(active, disks.first())
        assertEquals("primary", disks.last().location)
        assertEquals(20, disks.last().gib)
        assertNotEquals(excluded.incarnation, disks.last().incarnation)
    }
}
