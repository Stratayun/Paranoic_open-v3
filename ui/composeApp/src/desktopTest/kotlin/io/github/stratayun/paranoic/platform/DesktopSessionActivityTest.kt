package io.github.stratayun.paranoic.platform

import kotlin.test.Test
import kotlin.test.assertEquals

/** Prepared regression scenarios. No display, user files or real key store. */
class DesktopSessionActivityTest {
    private class TestWindow(val owner: TestWindow? = null)

    @Test fun ownDialogsKeepTheSessionButLeavingADialogLocks() {
        val root = TestWindow()
        val passwordDialog = TestWindow(root)
        val nestedDialog = TestWindow(passwordDialog)
        val otherApplication = TestWindow()
        val events = mutableListOf<String>()
        val state = DesktopSessionActivity(root, { it.owner }, { events += "resume" }, { events += "lock" })
        state.update(root, true, false)
        state.update(passwordDialog, true, false)
        state.update(nestedDialog, true, false)
        state.update(root, true, false)
        assertEquals(listOf("resume"), events)
        state.update(passwordDialog, true, false)
        state.update(otherApplication, true, false)
        state.update(null, true, false)
        assertEquals(listOf("resume", "lock"), events)
        state.update(root, true, false)
        assertEquals(listOf("resume", "lock", "resume"), events)
    }

    @Test fun minimizationLocksOnceAndRestoreWaitsForFocus() {
        val root = TestWindow()
        val dialog = TestWindow(root)
        val events = mutableListOf<String>()
        val state = DesktopSessionActivity(root, { it.owner }, { events += "resume" }, { events += "lock" })
        state.update(root, true, false)
        // Window focus can still name an owned window during iconification.
        state.update(dialog, true, true)
        state.update(null, true, true)
        state.update(null, true, false)
        assertEquals(listOf("resume", "lock"), events)
        state.update(root, true, false)
        assertEquals(listOf("resume", "lock", "resume"), events)
    }

    @Test fun disposedWindowCannotBeResumedByQueuedFocusCallbacks() {
        val root = TestWindow()
        val events = mutableListOf<String>()
        val state = DesktopSessionActivity(root, { it.owner }, { events += "resume" }, { events += "lock" })
        state.update(root, true, false)
        state.dispose()
        state.update(root, true, false)
        state.dispose()
        assertEquals(listOf("resume", "lock"), events)
    }

    @Test fun hiddenOrUnfocusedStartupCannotExposeAnOldSession() {
        val root = TestWindow()
        val events = mutableListOf<String>()
        val state = DesktopSessionActivity(root, { it.owner }, { events += "resume" }, { events += "lock" })
        state.update(root, false, false)
        state.update(null, true, false)
        state.dispose()
        assertEquals(listOf("lock"), events)
    }
}
