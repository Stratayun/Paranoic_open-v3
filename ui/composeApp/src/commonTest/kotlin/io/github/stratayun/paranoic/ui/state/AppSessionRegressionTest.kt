package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AppSessionRegressionTest {
    private val root = Location("storage", "")
    private val space = Space("storage", "Files", root, true)
    private fun entry(name: String) = FileItem(Location("storage", name), name, EntryKind.File, 1uL)
    private class FakeCore : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var listing: suspend (Location) -> CoreResult<FolderListing> = { CoreResult.Value(FolderListing(emptyList())) }
        var create: suspend () -> CoreResult<JobItem> = { CoreResult.Value(JobItem("job", JobPhase.Queued)) }
        override suspend fun list(folder: Location) = listing(folder)
        override suspend fun createDirectory(parent: Location, name: String, requestId: String) = create()
        override suspend fun spaces() = CoreResult.Value(emptyList<Space>())
        override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
    }

    @Test fun lateFolderResponseCannotReplaceNewerFolderOrHiddenCount() = runTest {
        val old = CompletableDeferred<CoreResult<FolderListing>>()
        val core = FakeCore().apply { listing = { if (it == root) old.await() else CoreResult.Value(FolderListing(listOf(entry("new")), 3)) } }
        val session = AppSession(core, backgroundScope)
        session.openSpace(space); runCurrent()
        session.openFolder(FileItem(Location("storage", "sub"), "sub", EntryKind.Directory, 0uL)); runCurrent()
        old.complete(CoreResult.Value(FolderListing(listOf(entry("old")), 99))); runCurrent()
        assertEquals("sub", session.files.folder?.path)
        assertEquals(listOf("new"), session.files.entries.map { it.name })
        assertEquals(3L, session.files.hidden)
        session.pause()
    }

    @Test fun pauseDiscardsEvenNonCooperativeReadAndClearsSelection() = runTest {
        val late = CompletableDeferred<CoreResult<FolderListing>>()
        val core = FakeCore()
        val session = AppSession(core, backgroundScope)
        core.listing = { CoreResult.Value(FolderListing(listOf(entry("before")))) }
        session.openSpace(space); runCurrent()
        session.select(entry("before").location)
        assertEquals(1, session.files.selected.size)
        core.listing = { withContext(NonCancellable) { late.await() } }
        session.refresh(); runCurrent(); session.pause()
        late.complete(CoreResult.Value(FolderListing(listOf(entry("late"))))); runCurrent()
        assertEquals(FilesState(), session.files)
        assertTrue(session.jobs.isEmpty())
    }

    @Test fun oldWriteFinallyCannotReleaseNewEpochWriteGuard() = runTest {
        val old = CompletableDeferred<CoreResult<JobItem>>()
        val new = CompletableDeferred<CoreResult<JobItem>>()
        var calls = 0
        val core = FakeCore().apply { create = {
            val pending = if (++calls == 1) old else new
            withContext(NonCancellable) { pending.await() }
        } }
        val session = AppSession(core, backgroundScope)
        session.createDirectory(root, "first"); runCurrent(); session.pause()
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        session.createDirectory(root, "second"); runCurrent()
        old.complete(CoreResult.Value(JobItem("old", JobPhase.Succeeded))); runCurrent()
        assertTrue(session.writing)
        assertTrue(session.jobs.isEmpty())
        session.createDirectory(root, "third"); runCurrent()
        assertEquals(2, calls, "duplicate write dispatched while newer write was pending")
        new.complete(CoreResult.Value(JobItem("new", JobPhase.Queued))); runCurrent()
        assertFalse(session.writing)
        assertEquals(listOf(JobItem("new", JobPhase.Queued)), session.jobs)
        session.pause()
    }

    @Test fun writeExceptionIsUnknownOutcomeAndNeverAutomaticallyRetried() = runTest {
        var calls = 0
        val core = FakeCore().apply { create = { ++calls; error("lost response") } }
        val session = AppSession(core, backgroundScope)
        session.createDirectory(root, "new"); runCurrent()
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        assertTrue(session.jobs.isEmpty())
        session.resume(); session.refresh(); runCurrent()
        assertEquals(1, calls)
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        session.pause()
    }

    @Test fun failedListingIsNotPresentedAsSuccessfulEmptyFolder() = runTest {
        val core = FakeCore().apply { listing = { CoreResult.Failure(CoreFault.Permission) } }
        val session = AppSession(core, backgroundScope)
        session.openSpace(space); runCurrent()
        assertFalse(session.files.loading)
        assertFalse(session.files.loaded)
        assertEquals(CoreFault.Permission, session.files.fault)
        session.select(Location("storage", "absent"))
        assertTrue(session.files.selected.isEmpty())
        session.pause()
    }

    @Test fun disconnectedGatewayCannotDispatchWrites() = runTest {
        var calls = 0
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override suspend fun createDirectory(parent: Location, name: String, requestId: String): CoreResult<JobItem> {
                calls++; return CoreResult.Value(JobItem("invalid", JobPhase.Succeeded))
            }
        }
        val session = AppSession(core, backgroundScope)
        session.createDirectory(root, "new"); runCurrent()
        assertEquals(0, calls)
        assertFalse(session.writing)
        session.pause()
    }

    @Test fun uncertainWriteIsCheckedByRequestIdAndResentOnlyWithTheSameId() = runTest {
        val ids = mutableListOf<String>()
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun createDirectory(parent: Location, name: String, requestId: String): CoreResult<JobItem> {
                ids += requestId
                return if (ids.size == 1) error("lost response") else CoreResult.Value(JobItem(requestId, JobPhase.Queued))
            }
            override suspend fun job(id: String) = CoreResult.Value<JobItem?>(null)
            override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(emptyList()))
            override suspend fun spaces() = CoreResult.Value(emptyList<Space>())
        }
        val session = AppSession(core, backgroundScope, newRequestId = { "request-1" })
        session.createDirectory(root, "new"); runCurrent()
        assertEquals("request-1", session.uncertain?.requestId)
        session.resendUncertain(); runCurrent()
        assertEquals(1, ids.size, "resent before the journal was consulted")
        session.checkUncertain(); runCurrent()
        assertEquals(true, session.uncertain?.notInJournal)
        session.resendUncertain(); runCurrent()
        assertEquals(listOf("request-1", "request-1"), ids)
        assertNull(session.uncertain)
        assertEquals("request-1", session.jobs.single().id)
        session.pause()
    }
}
