package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlin.test.*

class VaultFolderPickerFlowTest {
    private val old = FolderNode(Location("old", "selected"), "Selected")
    private val new = FolderNode(Location("new", ""), "Connected")
    private fun request() = VaultFolderRequest(true, VaultKind.Cryptomator, listOf(old))

    @Test fun selectedRootReplacesOldTrailAndKeepsCreationFormat() {
        val flow = VaultFolderPickerFlow()
        assertTrue(flow.begin(request()))
        val ticket = assertNotNull(flow.acceptSelection())
        val returned = assertNotNull(flow.finish(ticket, new))
        assertEquals(listOf(new), returned.request.trail)
        assertTrue(returned.request.creating)
        assertEquals(VaultKind.Cryptomator, returned.request.kind)
        assertNull(returned.fault)
        assertNull(flow.pending)
    }

    @Test fun cancelAndFailureKeepThePreviousFolder() {
        for (fault in listOf(null, CoreFault.Permission, CoreFault.OutcomeUnknown)) {
            val flow = VaultFolderPickerFlow()
            flow.begin(request())
            val ticket = assertNotNull(flow.acceptSelection())
            val returned = assertNotNull(flow.finish(ticket, folder = if (fault != null) new else null, fault = fault))
            assertEquals(listOf(old), returned.request.trail)
            assertEquals(fault, returned.fault)
        }
    }

    @Test fun relocationPreservesOriginalCardUntilUnlock() {
        val saved = SavedVault(old.location, VaultKind.Paranoic, "My vault")
        val flow = VaultFolderPickerFlow()
        flow.begin(VaultFolderRequest(false, saved.kind, listOf(old), saved))
        val returned = assertNotNull(flow.finish(assertNotNull(flow.acceptSelection()), new))
        assertFalse(returned.request.creating)
        assertEquals(saved, returned.request.relocating)
        assertEquals(old.location, returned.request.relocating?.location)
        assertEquals(listOf(new), returned.request.trail)
    }

    @Test fun duplicateSelectionAndOverlappingRequestsAreIgnored() {
        val flow = VaultFolderPickerFlow()
        assertTrue(flow.begin(request()))
        val ticket = assertNotNull(flow.acceptSelection())
        assertNull(flow.acceptSelection())
        assertFalse(flow.begin(request()))
        assertNotNull(flow.finish(ticket))
        assertNull(flow.finish(ticket))
    }

    @Test fun oldReplyCannotConsumeEqualNewRequest() {
        val flow = VaultFolderPickerFlow()
        flow.begin(request())
        val oldTicket = assertNotNull(flow.acceptSelection())
        flow.finish(oldTicket)
        flow.begin(request())
        val newTicket = assertNotNull(flow.acceptSelection())
        assertEquals(oldTicket, newTicket)
        assertNull(flow.finish(oldTicket, new))
        assertSame(newTicket, flow.pending)
        assertNotNull(flow.finish(newTicket, new))
    }

    @Test fun callerCannotMutateThePendingTrail() {
        val trail = mutableListOf(old)
        val flow = VaultFolderPickerFlow()
        flow.begin(request().copy(trail = trail))
        trail.clear()
        assertEquals(listOf(old), assertNotNull(flow.pending).trail)
    }
}
