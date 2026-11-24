package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.*

/** Prepared source; execution remains a separate verification task. */
@OptIn(ExperimentalCoroutinesApi::class)
class ArchiveRegressionTest {
    private val root = Location("files", "")
    private val space = Space("files", "Files", root, true)
    private val source = FileItem(Location("files", "input.txt"), "input.txt", EntryKind.File, 5uL)
    @Test fun closingAnArchiveDropsALateUncooperativeListing() = runTest {
        val late = CompletableDeferred<CoreResult<List<ArchiveEntry>>>()
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override suspend fun archiveList(source: Location, format: ArchiveFormat, password: String?) =
                withContext(NonCancellable) { late.await() }
        }
        val archive = ArchiveSession(core, backgroundScope)
        archive.open(source, ArchiveFormat.SevenZip, "password"); runCurrent(); archive.close()
        late.complete(CoreResult.Value(listOf(ArchiveEntry("secret.txt", false, 1uL, 1uL)))); runCurrent()
        assertEquals(ArchiveState(), archive.state)
    }
    @Test fun staleSelectionAndEncryptedSevenZipCreationNeverReachCore() = runTest {
        var creates = 0
        var items = listOf(source)
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun spaces() = CoreResult.Value(listOf(space))
            override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(items))
            override suspend fun archiveCreate(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String): CoreResult<Unit> {
                creates++; return CoreResult.Value(Unit)
            }
        }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.SevenZip, "password", root, "new.7z"); runCurrent()
        assertEquals(CoreFault.Unsupported, session.writeFault)
        items = emptyList(); session.refresh(); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.Zip, null, root, "new.zip"); runCurrent()
        assertEquals(CoreFault.Changed, session.writeFault)
        assertEquals(0, creates)
        session.pause()
    }
    @Test fun lostArchiveCreationReplyDoesNotCreateAnotherArchiveOnResume() = runTest {
        var creates = 0
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun spaces() = CoreResult.Value(listOf(space))
            override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(listOf(source)))
            override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
            override suspend fun archiveCreate(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String): CoreResult<Unit> {
                creates++; error("lost reply")
            }
        }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.Zip, null, root, "new.zip"); runCurrent()
        assertEquals(DirectWritePhase.Unknown, session.directWrite?.phase)
        session.resume(); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.Zip, null, root, "another.zip"); runCurrent()
        assertEquals(1, creates)
        assertNull(session.uncertain, "an archive preparation is never replayed automatically")
        session.pause()
    }
    @Test fun archiveCancellationUsesThePublicationRequestIdentity() = runTest {
        val reply = CompletableDeferred<CoreResult<Unit>>()
        var entered: String? = null
        var cancelled: String? = null
        val core = object : CoreGateway by DisconnectedCoreGateway, ArchiveGateway {
            override val connected = true
            override suspend fun spaces() = CoreResult.Value(listOf(space))
            override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(listOf(source)))
            override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
            override suspend fun archiveListWithRequest(source: Location, format: ArchiveFormat, password: String?, requestId: String) =
                CoreResult.Value(emptyList<ArchiveEntry>())
            override suspend fun archiveExtractWithRequest(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String): CoreResult<Unit> =
                CoreResult.Failure(CoreFault.Unsupported)
            override suspend fun archiveCreateWithRequest(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String): CoreResult<Unit> {
                entered = requestId; return reply.await()
            }
            override suspend fun cancelArchiveRequest(requestId: String): CoreResult<Unit> {
                cancelled = requestId; return CoreResult.Value(Unit)
            }
        }
        val session = AppSession(core, backgroundScope) { "archive-request" }
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.Zip, null, root, "new.zip"); runCurrent()
        session.cancelDirectArchive(); runCurrent()
        assertEquals(entered, cancelled)
        assertEquals("archive-request", cancelled)
        assertTrue(session.directWrite?.cancelling == true)
        reply.complete(CoreResult.Failure(CoreFault.Cancelled)); runCurrent()
        assertEquals(CoreFault.Cancelled, session.writeFault)
        session.pause()
    }

    @Test fun cancellationBeforeDispatchDoesNotStartArchivePreparation() = runTest {
        var creates = 0
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun spaces() = CoreResult.Value(listOf(space))
            override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(listOf(source)))
            override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
            override suspend fun archiveCreate(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String): CoreResult<Unit> {
                creates++; return CoreResult.Value(Unit)
            }
        }
        val session = AppSession(core, backgroundScope) { "cancel-before-dispatch" }
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.createArchive(listOf(source), ArchiveFormat.Zip, null, root, "new.zip")
        session.cancelDirectArchive(); runCurrent()
        assertEquals(0, creates)
        assertEquals(CoreFault.Cancelled, session.writeFault)
        session.pause()
    }

}
