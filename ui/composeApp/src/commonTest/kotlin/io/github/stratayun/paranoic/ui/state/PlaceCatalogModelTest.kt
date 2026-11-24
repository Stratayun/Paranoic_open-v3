package io.github.stratayun.paranoic.ui.state

import kotlin.test.*

class PlaceCatalogModelTest {
    @Test fun renameIsNavigationMetadataAndSurvivesHideUndo() {
        val named = PlaceCatalogSnapshot().rename("grant", "  Работа  ")
        val (hidden, removal) = named.hide("grant")
        assertEquals(PlacePreference("grant", "Работа", true), hidden.find("grant"))
        assertEquals(named, hidden.undo(removal))
    }
    @Test fun undoRestoresTheAbsenceOfAnOriginalPreference() {
        val initial = PlaceCatalogSnapshot()
        val (hidden, removal) = initial.hide("grant")
        assertEquals(initial, hidden.undo(removal))
        assertTrue(hidden.show("grant").entries.isEmpty())
    }
    @Test fun undoPreservesChangesToOtherPlaces() {
        val (hidden, removal) = PlaceCatalogSnapshot().hide("one")
        val renamed = hidden.rename("two", "Second")
        val undone = renamed.undo(removal)
        assertNull(undone.find("one"))
        assertEquals("Second", undone.find("two")?.title)
    }
    @Test fun showingAHiddenPlaceKeepsItsCustomTitle() {
        val (hidden, _) = PlaceCatalogSnapshot().rename("grant", "USB").hide("grant")
        assertEquals(PlacePreference("grant", "USB", false), hidden.show("grant").find("grant"))
    }
    @Test fun undoRefusesToReplaceANewerDecision() {
        val (hidden, removal) = PlaceCatalogSnapshot().hide("grant")
        assertFails { hidden.rename("grant", "New name").undo(removal) }
        assertFails { hidden.show("grant").undo(removal) }
        assertFails { hidden.hide("grant") }
    }
    @Test fun duplicateIdsAndUnsafeTitlesAreRejected() {
        assertFails { PlaceCatalogSnapshot(listOf(PlacePreference("id"), PlacePreference("id"))) }
        for (title in listOf(" ", "x\nname", "a".repeat(121)))
            assertFails { PlaceCatalogSnapshot().rename("id", title) }
        assertFails { PlacePreference("bad\nid") }
        assertFails { PlaceCatalogSnapshot(List(257) { PlacePreference("id$it") }) }
    }
}
