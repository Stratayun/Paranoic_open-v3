package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.*
import kotlin.test.*

class FileBatchTest {
    private val parent = Location("disk", "to")
    private val sources = listOf("a", "b", "c").map { FileItem(Location("disk", "from/$it"), it, EntryKind.File, 10uL) }
    private val space = Space("disk", "Disk", Location("disk", ""), true)
    private fun copy() = FileBatchPlan(FileBatchKind.Copy, sources, parent)
    private class Gateway : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        val calls = mutableListOf<Pair<Location, String>>()
        var transfer: suspend (String) -> CoreResult<JobItem> = { CoreResult.Value(JobItem(it, JobPhase.Queued)) }
        val recursiveFlags = mutableListOf<Boolean>()
        override suspend fun transfer(source: Location, parent: Location, name: String, mode: TransferMode,
            conflict: NameConflict, requestId: String): CoreResult<JobItem> {
            calls += source to requestId
            return transfer(requestId)
        }
        override suspend fun delete(source: Location, recursive: Boolean, requestId: String): CoreResult<JobItem> {
            calls += source to requestId; recursiveFlags += recursive
            return transfer(requestId)
        }
    }
    private val ids = listOf("one", "two", "three")

    @Test fun requestIdsAreFixedBeforeTheFirstSubmission() = runBlocking {
        val gateway = Gateway(); val accepted = mutableListOf<String>(); var report: FileBatchReport? = null
        gateway.transfer = { id ->
            assertEquals(ids, assertNotNull(report).entries.map { it.requestId })
            CoreResult.Value(JobItem(id, JobPhase.Queued))
        }
        FileBatchDispatcher(gateway).send(copy(), ids, { false }, { report = it }, { accepted += it.id })
        assertEquals(ids, gateway.calls.map { it.second }); assertEquals(ids, accepted)
        assertFalse(assertNotNull(report).sending)
    }
    @Test fun unknownReplyStopsRemainingSubmissionsWithoutRetry() = runBlocking {
        val gateway = Gateway(); var report: FileBatchReport? = null
        gateway.transfer = { id -> if (id == "two") CoreResult.Failure(CoreFault.OutcomeUnknown) else CoreResult.Value(JobItem(id, JobPhase.Queued)) }
        FileBatchDispatcher(gateway).send(copy(), ids, { false }, { report = it }, {})
        assertEquals(listOf("one", "two"), gateway.calls.map { it.second })
        assertEquals(listOf(BatchDelivery.Accepted, BatchDelivery.Unknown, BatchDelivery.NotSent), assertNotNull(report).entries.map { it.delivery })
    }
    @Test fun unexpectedExceptionIsAnUnknownOutcome() = runBlocking {
        val gateway = Gateway(); var report: FileBatchReport? = null
        gateway.transfer = { error("lost transport") }
        FileBatchDispatcher(gateway).send(copy(), ids, { false }, { report = it }, {})
        assertEquals(1, gateway.calls.size)
        assertEquals(BatchDelivery.Unknown, assertNotNull(report).entries.first().delivery)
    }
    @Test fun knownRefusalDoesNotHideTheOtherResults() = runBlocking {
        val gateway = Gateway(); var report: FileBatchReport? = null
        gateway.transfer = { id -> if (id == "two") CoreResult.Failure(CoreFault.Permission) else CoreResult.Value(JobItem(id, JobPhase.Queued)) }
        FileBatchDispatcher(gateway).send(copy(), ids, { false }, { report = it }, {})
        assertEquals(3, gateway.calls.size)
        assertEquals(listOf(BatchDelivery.Accepted, BatchDelivery.Rejected, BatchDelivery.Accepted), assertNotNull(report).entries.map { it.delivery })
    }
    @Test fun stopDoesNotCancelAnAlreadyAcceptedJob() = runBlocking {
        val gateway = Gateway(); var stop = false; var report: FileBatchReport? = null
        FileBatchDispatcher(gateway).send(copy(), ids, { stop }, { report = it }, { stop = true })
        assertEquals(1, gateway.calls.size)
        assertEquals(BatchDelivery.Accepted, assertNotNull(report).entries.first().delivery)
        assertEquals(2, assertNotNull(report).entries.count { it.delivery == BatchDelivery.NotSent })
    }
    @Test fun cancellationKeepsTheUncertainRequestAndDropsUnsentEntries() = runBlocking {
        val gateway = Gateway(); val started = CompletableDeferred<Unit>(); var report: FileBatchReport? = null
        gateway.transfer = { started.complete(Unit); awaitCancellation() }
        val sender = launch { FileBatchDispatcher(gateway).send(copy(), ids, { false }, { report = it }, {}) }
        started.await(); sender.cancelAndJoin()
        val interrupted = assertNotNull(report)
        assertEquals(BatchDelivery.Unknown, interrupted.entries.first().delivery)
        assertEquals(2, interrupted.entries.count { it.delivery == BatchDelivery.NotSent })
        assertFalse(interrupted.sending)
        assertTrue(interrupted.redact().entries.all { it.source == null })
        assertEquals(ids, interrupted.redact().entries.map { it.requestId })
    }
    @Test fun journalReconciliationNeverTurnsAbsenceIntoAReplay() {
        val unknown = FileBatchReport(FileBatchKind.Copy, listOf(FileBatchEntry("id", sources.first(), BatchDelivery.Unknown)), false)
        assertEquals(unknown, unknown.reconcile(emptyList()))
        val found = unknown.reconcile(listOf(JobItem("id", JobPhase.Running)))
        assertEquals(BatchDelivery.Accepted, found.entries.single().delivery)
        assertEquals("id", found.entries.single().jobId)
    }
    @Test fun summarySeparatesSuccessSkippingFailureAndUnknown() {
        val report = FileBatchReport(FileBatchKind.Copy, listOf(
            FileBatchEntry("one", null, BatchDelivery.Accepted, "one"),
            FileBatchEntry("two", null, BatchDelivery.Accepted, "two"),
            FileBatchEntry("three", null, BatchDelivery.Rejected, fault = CoreFault.Permission),
            FileBatchEntry("four", null, BatchDelivery.Unknown),
            FileBatchEntry("five", null, BatchDelivery.NotSent)), false)
        val summary = report.summary(listOf(JobItem("one", JobPhase.Succeeded, outcome = JobOutcome.Done),
            JobItem("two", JobPhase.Succeeded, outcome = JobOutcome.Skipped)))
        assertEquals(FileBatchSummary(5, 1, 1, 1, 0, 0, 1, 1), summary)
        assertEquals(4, summary.settled)
    }
    @Test fun batchPoliciesDoNotAllowReplacementOrUnsafeDestinations() {
        assertTrue(copy().allowed(listOf(space)))
        assertFails { copy().copy(conflict = NameConflict.Replace) }
        assertFails { FileBatchPlan(FileBatchKind.Move, sources, parent, NameConflict.Skip) }
        assertFalse(copy().copy(parent = Location("disk", "from")).allowed(listOf(space)))
        assertTrue(copy().copy(parent = Location("disk", "from"), conflict = NameConflict.KeepBoth).allowed(listOf(space)))
        val folder = FileItem(Location("disk", "from/folder"), "folder", EntryKind.Directory, 0uL)
        assertFalse(FileBatchPlan(FileBatchKind.Copy, listOf(folder), Location("disk", "from/folder/sub")).allowed(listOf(space)))
    }
    @Test fun recursiveDeletionIsSentOnlyForDirectories() = runBlocking {
        val gateway = Gateway()
        val folder = FileItem(Location("disk", "from/folder"), "folder", EntryKind.Directory, 0uL)
        FileBatchDispatcher(gateway).send(FileBatchPlan(FileBatchKind.Delete, listOf(sources.first(), folder), recursive = true),
            ids.take(2), { false }, {}, {})
        assertEquals(listOf(false, true), gateway.recursiveFlags)
    }
    @Test fun callerMutationDoesNotChangeAnInFlightSelection() = runBlocking {
        val gateway = Gateway(); val mutableSources = sources.toMutableList(); val mutableIds = ids.toMutableList()
        gateway.transfer = { id -> mutableSources.clear(); mutableIds.clear(); CoreResult.Value(JobItem(id, JobPhase.Queued)) }
        FileBatchDispatcher(gateway).send(copy().copy(sources = mutableSources), mutableIds, { false }, {}, {})
        assertEquals(sources.map { it.location }, gateway.calls.map { it.first })
        assertEquals(ids, gateway.calls.map { it.second })
    }
    @Test fun duplicateOrOversizedSelectionsAreRejectedBeforeSubmission() {
        assertFails { copy().copy(sources = listOf(sources.first(), sources.first())) }
        assertFails { copy().copy(sources = emptyList()) }
        assertFails { copy().copy(sources = (0..MAX_FILE_SELECTION).map {
            FileItem(Location("disk", "from/$it"), "$it", EntryKind.File, 0uL) }) }
        assertFails { FileBatchReport(FileBatchKind.Copy, listOf(FileBatchEntry("same", null), FileBatchEntry("same", null))) }
    }
}
