package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.*

/** Prepared regression scenarios; running them requires the owner's verification command. */
@OptIn(ExperimentalCoroutinesApi::class)
class FileOperationsRegressionTest {
    private val root = Location("source", "")
    private val targetRoot = Location("target", "")
    private val sourceSpace = Space("source", "Source", root, true)
    private val targetSpace = Space("target", "Target", targetRoot, true)
    private val file = FileItem(Location("source", "file.txt"), "file.txt", EntryKind.File, 8uL, 1uL)
    private val folder = FileItem(Location("source", "folder"), "folder", EntryKind.Directory, 0uL)
    private val existing = FileItem(Location("target", "file.txt"), "file.txt", EntryKind.File, 4uL, 2uL)
    private data class Transfer(val id: String, val source: Location, val parent: Location, val name: String,
        val mode: TransferMode, val conflict: NameConflict)
    private class Core : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var registered = emptyList<Space>()
        var entries = emptyList<FileItem>()
        val transfers = mutableListOf<Transfer>()
        val renames = mutableListOf<Triple<Location, String, String>>()
        val deletions = mutableListOf<Triple<Location, Boolean, String>>()
        var reply: suspend (String) -> CoreResult<JobItem> = { CoreResult.Value(JobItem(it, JobPhase.Succeeded)) }
        override suspend fun spaces() = CoreResult.Value(registered)
        override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(entries))
        override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
        override suspend fun job(id: String) = CoreResult.Value<JobItem?>(null)
        override suspend fun transfer(source: Location, parent: Location, name: String, mode: TransferMode,
            conflict: NameConflict, requestId: String): CoreResult<JobItem> {
            transfers += Transfer(requestId, source, parent, name, mode, conflict)
            return reply(requestId)
        }
        override suspend fun rename(source: Location, name: String, requestId: String): CoreResult<JobItem> {
            renames += Triple(source, name, requestId)
            return reply(requestId)
        }
        override suspend fun delete(source: Location, recursive: Boolean, requestId: String): CoreResult<JobItem> {
            deletions += Triple(source, recursive, requestId)
            return reply(requestId)
        }
    }

    @Test fun replacementNeedsExplicitFileTargetAndMovesNeverAcceptOverwritePolicies() = runTest {
        val core = Core().apply { registered = listOf(sourceSpace, targetSpace); entries = listOf(file) }
        val session = AppSession(core, backgroundScope, newRequestId = { "replace-1" })
        session.loadSpaces(); session.openSpace(sourceSpace); runCurrent()
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Replace)
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Replace,
            existing.copy(kind = EntryKind.Directory))
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Replace,
            existing.copy(location = Location("target", "another.txt")))
        session.transfer(file, root, file.name, TransferMode.Copy, NameConflict.Replace, file)
        session.transfer(file, targetRoot, file.name, TransferMode.Move, NameConflict.KeepBoth)
        session.transfer(file, targetRoot, file.name, TransferMode.Move, NameConflict.Replace, existing)
        runCurrent()
        assertTrue(core.transfers.isEmpty())
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Replace, existing); runCurrent()
        assertEquals(listOf(Transfer("replace-1", file.location, targetRoot, file.name, TransferMode.Copy, NameConflict.Replace)), core.transfers)
        session.pause()
    }

    @Test fun readonlySourceCanBeCopiedButCannotBeRenamedMovedOrDeleted() = runTest {
        val readonly = sourceSpace.copy(writable = false)
        val core = Core().apply { registered = listOf(readonly, targetSpace); entries = listOf(file) }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(readonly); runCurrent()
        session.delete(file); session.rename(file, "new.txt")
        session.transfer(file, targetRoot, file.name, TransferMode.Move, NameConflict.Fail); runCurrent()
        assertTrue(core.deletions.isEmpty()); assertTrue(core.renames.isEmpty()); assertTrue(core.transfers.isEmpty())
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.KeepBoth); runCurrent()
        assertEquals(NameConflict.KeepBoth, core.transfers.single().conflict)
        session.pause()
    }

    @Test fun staleSourceAndDirectoryDescendantsNeverReachCore() = runTest {
        val core = Core().apply { registered = listOf(sourceSpace, targetSpace); entries = listOf(file, folder) }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(sourceSpace); runCurrent()
        session.transfer(folder, folder.location, "copy", TransferMode.Copy, NameConflict.Fail)
        session.transfer(folder, Location("source", "folder/sub"), "copy", TransferMode.Move, NameConflict.Fail)
        core.entries = listOf(file.copy(modifiedMs = 3uL), folder)
        session.refresh(); runCurrent()
        session.delete(file); session.rename(file, "new.txt")
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Fail); runCurrent()
        assertEquals(CoreFault.Changed, session.writeFault)
        assertTrue(core.deletions.isEmpty()); assertTrue(core.renames.isEmpty()); assertTrue(core.transfers.isEmpty())
        session.pause()
    }

    @Test fun lostTransferReplyKeepsOriginalPolicyAndIdUntilExplicitJournalCheckAndResend() = runTest {
        val core = Core().apply {
            registered = listOf(sourceSpace, targetSpace); entries = listOf(file)
            reply = { id -> if (transfers.size == 1) error("lost reply") else CoreResult.Value(JobItem(id, JobPhase.Succeeded)) }
        }
        val session = AppSession(core, backgroundScope, newRequestId = { "transfer-1" })
        session.loadSpaces(); session.openSpace(sourceSpace); runCurrent()
        session.transfer(file, targetRoot, file.name, TransferMode.Copy, NameConflict.Skip); runCurrent()
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        session.refresh(); session.resendUncertain(); runCurrent()
        assertEquals(1, core.transfers.size)
        session.checkUncertain(); runCurrent()
        session.resendUncertain(); runCurrent()
        assertEquals(2, core.transfers.size)
        assertEquals(core.transfers.first(), core.transfers.last())
        assertNull(session.uncertain)
        session.pause()
    }

    @Test fun pausedRecursiveDeleteKeepsItsPayloadAndIgnoresLateReply() = runTest {
        val late = CompletableDeferred<CoreResult<JobItem>>()
        val core = Core().apply {
            registered = listOf(sourceSpace); entries = listOf(folder)
            reply = { withContext(NonCancellable) { late.await() } }
        }
        val session = AppSession(core, backgroundScope, newRequestId = { "delete-1" })
        session.loadSpaces(); session.openSpace(sourceSpace); runCurrent()
        session.delete(folder, recursive = true); runCurrent(); session.pause()
        assertEquals("delete-1", session.uncertain?.requestId)
        late.complete(CoreResult.Value(JobItem("delete-1", JobPhase.Succeeded))); runCurrent()
        assertTrue(session.jobs.isEmpty())
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        core.reply = { CoreResult.Value(JobItem(it, JobPhase.Succeeded)) }
        session.checkUncertain(); runCurrent(); session.resendUncertain(); runCurrent()
        assertEquals(listOf(Triple(folder.location, true, "delete-1"), Triple(folder.location, true, "delete-1")), core.deletions)
        session.pause()
    }
}
