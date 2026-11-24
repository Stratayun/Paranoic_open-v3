package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlin.test.*

class VaultCatalogModelTest {
    private fun vault(path: String, title: String = path) = SavedVault(Location("grant", path), VaultKind.Paranoic, title)
    @Test fun rediscoveryPreservesCustomTitle() {
        val original = VaultCatalogSnapshot().remember(vault("one")).rename(Location("grant", "one"), "Work")
        assertEquals("Work", original.remember(vault("one", "Rediscovered")).entries.single().title)
        assertEquals(1, original.entries.size)
    }
    @Test fun removeAndUndoRestoreOrderAndTitle() {
        val first = vault("one", "Work"); val second = vault("two", "Personal")
        val snapshot = VaultCatalogSnapshot(listOf(first, second))
        val removed = snapshot.remove(first.location)
        assertEquals(listOf(second), removed.entries)
        assertEquals(snapshot, removed.restore(first, 0))
        assertEquals(listOf(first, second), snapshot.entries)
    }
    @Test fun sameLocationCannotSilentlyChangeFormat() {
        val entry = vault("one")
        assertFailsWith<IllegalArgumentException> {
            VaultCatalogSnapshot(listOf(entry)).remember(entry.copy(kind = VaultKind.Cryptomator))
        }
    }
    @Test fun disconnectedGrantsDoNotCollapseIntoSamePath() {
        val first = vault("one")
        val second = first.copy(location = Location("other-grant", "one"))
        assertEquals(2, VaultCatalogSnapshot().remember(first).remember(second).entries.size)
    }
    @Test fun duplicatesAndCapacityAreRejected() {
        val entry = vault("one")
        assertFailsWith<IllegalArgumentException> { VaultCatalogSnapshot(listOf(entry, entry)) }
        val full = VaultCatalogSnapshot((0 until 128).map { vault("v$it") })
        assertFailsWith<IllegalArgumentException> { full.remember(vault("overflow")) }
        assertEquals(128, full.entries.size)
    }
    @Test fun invalidTitlesNeverReachPersistence() {
        for (title in listOf("", "   ", "line\nbreak", "x".repeat(121))) {
            assertFalse(validVaultTitle(title))
            assertFailsWith<IllegalArgumentException> { vault("one", title) }
        }
        assertTrue(validVaultTitle("Работа · Work"))
    }
    @Test fun relocationPreservesTitleOrderAndDoesNotAddACard() {
        val first = vault("one", "Work"); val second = vault("two", "Personal")
        val original = VaultCatalogSnapshot(listOf(first, second))
        val changed = original.relocate(first, Location("new-grant", "moved"), first.kind)
        assertEquals(2, changed.entries.size)
        assertEquals(first.copy(location = Location("new-grant", "moved")), changed.entries.first())
        assertEquals(second, changed.entries[1])
        assertEquals(first, original.entries.first())
    }
    @Test fun relocationRefusesStaleCardOrMissingSource() {
        val first = vault("one", "Work")
        val renamed = VaultCatalogSnapshot(listOf(first)).rename(first.location, "Renamed")
        assertFailsWith<IllegalArgumentException> { renamed.relocate(first, Location("grant", "moved"), first.kind) }
        assertFailsWith<IllegalArgumentException> { VaultCatalogSnapshot().relocate(first, Location("grant", "moved"), first.kind) }
    }
    @Test fun relocationRefusesExistingTargetAndDifferentFormat() {
        val first = vault("one"); val second = vault("two")
        val original = VaultCatalogSnapshot(listOf(first, second))
        assertFailsWith<IllegalArgumentException> { original.relocate(first, second.location, first.kind) }
        assertFailsWith<IllegalArgumentException> { original.relocate(first, Location("grant", "moved"), VaultKind.Cryptomator) }
        assertEquals(listOf(first, second), original.entries)
    }
    @Test fun relocationToSameAddressIsIdempotent() {
        val first = vault("one"); val original = VaultCatalogSnapshot(listOf(first))
        assertEquals(original, original.relocate(first, first.location, first.kind))
    }
    @Test fun accessSettingsSurviveRenameAndRelocation() {
        val first = vault("one")
        val changed = VaultCatalogSnapshot(listOf(first)).configure(first.location, true, 60).rename(first.location, "Work")
        val saved = changed.entries.single()
        val moved = changed.relocate(saved, Location("new", "moved"), saved.kind).entries.single()
        assertTrue(moved.readOnly); assertEquals(60, moved.idleLockSeconds); assertEquals("Work", moved.title)
    }
    @Test fun timeoutCannotDisableOrUnboundLocking() {
        val first = vault("one")
        for (seconds in listOf(0, -1, 1, Int.MAX_VALUE)) {
            assertFails { first.copy(idleLockSeconds = seconds) }
            assertFails { VaultCatalogSnapshot(defaultIdleLockSeconds = seconds) }
        }
        assertEquals(300, VaultCatalogSnapshot().defaultIdleLockSeconds)
        assertNull(first.idleLockSeconds)
    }
    @Test fun rediscoveryDoesNotRelaxSavedAccessSettings() {
        val configured = vault("one").copy(readOnly = true, idleLockSeconds = 60)
        val snapshot = VaultCatalogSnapshot(listOf(configured), 900)
        assertEquals(snapshot, snapshot.remember(vault("one")))
        assertEquals(900, snapshot.remove(configured.location).defaultIdleLockSeconds)
    }

}
