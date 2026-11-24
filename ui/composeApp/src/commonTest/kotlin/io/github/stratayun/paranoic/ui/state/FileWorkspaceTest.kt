package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlin.test.*

class FileWorkspaceTest {
    private val root = Location("disk", "")
    private val space = Space("disk", "Disk", root, true)
    private val item = FileItem(Location("disk", "folder/file"), "file", EntryKind.File, 4uL)
    private fun populated(): FileWorkspace {
        val state = FilesState(listOf(space), listOf(FolderNode(Location("disk", "folder"), "Folder")),
            listOf(item), selected = setOf(item.location), loaded = true)
        return FileWorkspace().files("files-1", state)
    }
    @Test fun duplicateCopiesNavigationWithoutSelectionOrStaleEntries() {
        val workspace = populated().add(duplicate = true)
        val duplicate = workspace.current
        assertEquals(populated().current.files.trail, duplicate.files.trail)
        assertFalse(duplicate.files.loaded)
        assertTrue(duplicate.files.entries.isEmpty())
        assertTrue(duplicate.files.selected.isEmpty())
        assertEquals(setOf(item.location), workspace.tab("files-1")?.files?.selected)
    }
    @Test fun panelsHaveDifferentTabsAndKeepTheirOwnFilters() {
        var workspace = populated().split(true)
        val right = assertNotNull(workspace.right)
        workspace = workspace.update(right) { it.copy(options = FileListOptions(query = "right"), view = FileView.Detailed) }
        assertNotEquals(workspace.left, right)
        assertEquals("", workspace.tab(workspace.left)?.options?.query)
        assertEquals("right", workspace.tab(right)?.options?.query)
        assertEquals(right, workspace.focus(right).active)
        assertEquals(workspace.left, workspace.focus(workspace.left).active)
    }
    @Test fun closingAnActivePaneAlwaysLeavesAValidWorkspace() {
        val two = populated().split(true)
        val leftClosed = two.close(two.left)
        assertEquals(1, leftClosed.tabs.size)
        assertNotNull(leftClosed.tab(leftClosed.active))
        assertNull(leftClosed.right)
        assertEquals(leftClosed, leftClosed.close(leftClosed.active))
    }
    @Test fun moveAndCollapsePreserveTheFocusedTab() {
        val two = populated().split(true)
        val moved = two.moveToOtherPane(two.left)
        assertEquals(two.left, moved.right)
        assertEquals(two.left, moved.active)
        val collapsed = moved.split(false)
        assertEquals(two.left, collapsed.left)
        assertEquals(two.left, collapsed.active)
        assertNull(collapsed.right)
    }
    @Test fun updatingOneTabCannotReplaceAnotherTabsListing() {
        val two = populated().split(true)
        val right = assertNotNull(two.right)
        val changed = two.files(right, assertNotNull(two.tab(right)).files.copy(fault = CoreFault.Permission))
        assertEquals(listOf(item), changed.tab(two.left)?.files?.entries)
        assertEquals(CoreFault.Permission, changed.tab(right)?.files?.fault)
        assertNull(changed.tab(two.left)?.files?.fault)
    }
    @Test fun spaceChangesAndRevocationAffectEveryTab() {
        var workspace = populated().split(true)
        val readOnly = space.copy(writable = false)
        workspace = workspace.files(workspace.active, workspace.current.files.copy(spaces = listOf(readOnly)))
        assertTrue(workspace.tabs.all { it.files.spaces == listOf(readOnly) })
        val detached = workspace.withoutSpace(space.id)
        assertTrue(detached.tabs.all { it.files.folder == null && it.files.entries.isEmpty() && it.files.spaces.isEmpty() })
    }
    @Test fun limitAndCloseOthersDoNotReuseOldTabIds() {
        var workspace = FileWorkspace()
        repeat(MAX_FILE_TABS - 1) { workspace = workspace.add() }
        assertFails { workspace.add() }
        val oldIds = workspace.tabs.map { it.id }.toSet()
        workspace = workspace.keep(setOf(workspace.active)).add()
        assertTrue(workspace.active !in oldIds)
        assertFails { workspace.keep(emptySet()) }
    }
}
