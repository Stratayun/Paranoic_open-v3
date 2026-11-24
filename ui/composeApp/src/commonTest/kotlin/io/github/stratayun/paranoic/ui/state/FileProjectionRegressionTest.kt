package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlin.test.*

/** Cases adapted from v1.5 FileListProjectionTest; no legacy file engine. */
class FileProjectionRegressionTest {
    @Test fun unknownSizeIsVisibleAndSortsAfterKnownSizesInBothDirections() {
        val unknown = FileItem(Location("s", "unknown.txt"), "unknown.txt", EntryKind.File, 0uL, sizeKnown = false)
        val empty = item("s", "empty.txt", EntryKind.File, 0uL)
        val full = item("s", "full.txt", EntryKind.File, 20uL)
        val folder = item("s", "folder", EntryKind.Directory, 0uL)
        val input = listOf(unknown, full, empty, folder)
        assertEquals(listOf(folder, empty, full, unknown), visibleFiles(input, FileListOptions(order = FileOrder.SizeAscending)))
        assertEquals(listOf(folder, full, empty, unknown), visibleFiles(input, FileListOptions(order = FileOrder.SizeDescending)))
        assertFalse(unknown.sizeKnown)
        assertTrue(empty.sizeKnown)
    }
    private fun item(storage: String, name: String, kind: EntryKind, size: ULong) =
        FileItem(Location(storage, name), name, kind, size)

    @Test fun everyOrderKeepsDirectoriesFirstAndPreservesIdentity() {
        val small = item("s", "A.txt", EntryKind.File, 2uL)
        val large = item("s", "z.txt", EntryKind.File, ULong.MAX_VALUE)
        val directory = item("s", "middle", EntryKind.Directory, 0uL)
        val input = listOf(large, directory, small)
        val expected = mapOf(
            FileOrder.NameAscending to listOf(directory, small, large),
            FileOrder.NameDescending to listOf(directory, large, small),
            FileOrder.SizeAscending to listOf(directory, small, large),
            FileOrder.SizeDescending to listOf(directory, large, small),
        )
        expected.forEach { (order, items) ->
            val actual = visibleFiles(input, FileListOptions(order = order))
            assertEquals(items, actual)
            items.zip(actual).forEach { (original, projected) -> assertSame(original, projected) }
        }
        assertEquals(listOf(large, directory, small), input, "projection mutated source order")
    }

    @Test fun hiddenAndQueryFiltersComposeWithoutChangingLocations() {
        val visible = item("s", "Report.txt", EntryKind.File, 1uL)
        val hidden = item("s", ".report.txt", EntryKind.File, 1uL)
        val other = item("s", "image.png", EntryKind.File, 1uL)
        val input = listOf(other, hidden, visible)
        assertEquals(listOf(visible), visibleFiles(input, FileListOptions(query = "REPORT")))
        assertEquals(listOf(hidden, visible), visibleFiles(input, FileListOptions(query = "report", showHidden = true)))
    }

    @Test fun sameDisplayNameInDifferentStoragesDoesNotMergeSelection() {
        val a = item("first", "same.txt", EntryKind.File, 1uL)
        val b = item("second", "same.txt", EntryKind.File, 1uL)
        assertEquals(2, visibleFiles(listOf(a, b), FileListOptions()).size)
        assertEquals(2, setOf(a.location, b.location).size)
    }
}
