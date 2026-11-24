package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.*

/** Prepared scenarios only; no build or test run is implied. */
@OptIn(ExperimentalCoroutinesApi::class)
class FileCryptoRegressionTest {
    private val root = Location("local", "")
    private val space = Space("local", "Local", root, true, localContainer = true)
    private val file = FileItem(Location("local", "file.age"), "file.age", EntryKind.File, 8uL)
    private class Core : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var registered = emptyList<Space>()
        var entries = emptyList<FileItem>()
        var writes = 0
        var transform: suspend () -> CoreResult<Unit> = { CoreResult.Value(Unit) }
        override suspend fun spaces() = CoreResult.Value(registered)
        override suspend fun jobs() = CoreResult.Value(emptyList<JobItem>())
        override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(entries))
        override suspend fun decryptFile(source: Location, parent: Location, name: String, format: FileCryptoFormat,
            password: String): CoreResult<Unit> { writes++; return transform() }
    }

    @Test fun unknownWriteIsNeverResentByResumeOrAnotherSubmissionUntilAcknowledged() = runTest {
        val core = Core().apply { registered = listOf(space); entries = listOf(file); transform = { error("lost reply") } }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.transformFile(file, root, "plain.txt", FileCryptoFormat.Age, true, "password"); runCurrent()
        assertEquals(DirectWritePhase.Unknown, session.directWrite?.phase)
        assertNull(session.uncertain, "no request ID exists for this bridge call")
        session.resume(); runCurrent()
        session.transformFile(file, root, "another.txt", FileCryptoFormat.Age, true, "password"); runCurrent()
        assertEquals(1, core.writes)
        session.dismissDirectWrite()
        core.transform = { CoreResult.Value(Unit) }
        session.transformFile(file, root, "plain.txt", FileCryptoFormat.Age, true, "password"); runCurrent()
        assertEquals(2, core.writes)
        assertEquals(DirectWritePhase.Done, session.directWrite?.phase)
        session.pause()
    }

    @Test fun latePublicationReplyAfterPauseCannotClaimSuccessOrRestoreFileState() = runTest {
        val late = CompletableDeferred<CoreResult<Unit>>()
        val core = Core().apply {
            registered = listOf(space); entries = listOf(file)
            transform = { withContext(NonCancellable) { late.await() } }
        }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(space); runCurrent()
        session.transformFile(file, root, "plain.txt", FileCryptoFormat.Age, true, "password"); runCurrent()
        session.pause()
        late.complete(CoreResult.Value(Unit)); runCurrent()
        assertEquals(DirectWritePhase.Unknown, session.directWrite?.phase)
        assertEquals(FilesState(), session.files)
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        assertEquals(1, core.writes)
    }

    @Test fun samePathOutputIsRefusedBeforeNativeCall() = runTest {
        val remote = space.copy(localContainer = false)
        val core = Core().apply { registered = listOf(remote); entries = listOf(file) }
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.openSpace(remote); runCurrent()
        session.transformFile(file, root, file.name, FileCryptoFormat.Age, true, "password"); runCurrent()
        assertEquals(0, core.writes)
        session.pause()
    }
}
