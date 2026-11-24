package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.*

/** Prepared source only. No automatic test trigger is added. */
@OptIn(ExperimentalCoroutinesApi::class)
class AttentionRegressionTest {
    private val item = AttentionItem(JobItem("origin", JobPhase.NeedsAttention, kind = JobKind.Copy,
        source = Location("space", "a/source.txt"), target = Location("space", "b/target.txt")),
        AttentionReason.ReplacementBackup, "interrupted", Location("space", "b/.pnc-replaced-copy"), canDiscard = true)

    private class Core(initial: AttentionItem) : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var open = listOf(initial)
        var records = emptyList<JobItem>()
        var recent = emptyList<JobItem>()
        val submissions = mutableListOf<String>()
        val reviewed = mutableListOf<String>()
        var accepts = 0
        var submit: suspend (String) -> CoreResult<JobItem> = { CoreResult.Value(JobItem(it, JobPhase.Queued)) }
        var accept: suspend () -> CoreResult<Unit> = { CoreResult.Value(Unit) }
        var listing: suspend () -> CoreResult<List<AttentionItem>> = { CoreResult.Value(open) }
        override suspend fun spaces() = CoreResult.Value(listOf(Space("space", "Files", Location("space", ""), true)))
        override suspend fun list(folder: Location) = CoreResult.Value(FolderListing(emptyList()))
        override suspend fun jobs() = CoreResult.Value(recent)
        override suspend fun job(id: String) = CoreResult.Value(records.firstOrNull { it.id == id })
        override suspend fun attentionList() = listing()
        override suspend fun attentionDiscard(jobId: String, requestId: String): CoreResult<JobItem> {
            submissions += requestId
            return submit(requestId)
        }
        override suspend fun attentionReviewed(jobId: String): CoreResult<Unit> {
            reviewed += jobId
            open = open.filterNot { it.job.id == jobId }
            return CoreResult.Value(Unit)
        }
        override suspend fun attentionAccept(jobId: String): CoreResult<Unit> { accepts++; return accept() }
    }

    @Test fun queuedFailedCancelledAndAttentionJobsNeverReviewTheirOrigin() = runTest {
        for (phase in listOf(JobPhase.Failed, JobPhase.Cancelled, JobPhase.NeedsAttention)) {
            val core = Core(item)
            val session = AppSession(core, backgroundScope) { "recovery" }
            session.refreshAttention(); runCurrent()
            session.resolveAttention(item, AttentionAction.Discard); runCurrent()
            assertTrue(core.reviewed.isEmpty(), "accepting the submission is not completion")
            core.recent = listOf(JobItem("recovery", phase))
            session.refreshJobs(); runCurrent()
            assertTrue(core.reviewed.isEmpty(), "must not close origin after $phase")
            assertEquals(listOf(item), session.attention.items)
            session.pause()
        }
    }

    @Test fun successAfterPauseIsFoundByDurableIdOutsideRecentJobsAndReviewsOnlyOnce() = runTest {
        val core = Core(item)
        val session = AppSession(core, backgroundScope) { "recovery" }
        session.refreshAttention(); runCurrent()
        session.resolveAttention(item, AttentionAction.Discard); runCurrent(); session.pause()
        core.records = listOf(JobItem("recovery", JobPhase.Succeeded))
        // The recent window deliberately omits recovery; AppSession must query its ID.
        core.recent = emptyList()
        session.resume(); runCurrent()
        assertEquals(listOf("origin"), core.reviewed)
        assertTrue(session.attention.items.isEmpty())
        session.refreshJobs(); runCurrent()
        assertEquals(1, core.reviewed.size)
        assertEquals(listOf("recovery"), core.submissions)
        session.pause()
    }

    @Test fun lostDiscardReplyUsesJournalWithoutResubmittingAndStillClosesAfterConfirmedSuccess() = runTest {
        val core = Core(item).apply {
            submit = { id -> records = listOf(JobItem(id, JobPhase.Succeeded)); error("lost reply") }
        }
        val session = AppSession(core, backgroundScope) { "recovery" }
        session.refreshAttention(); runCurrent()
        session.resolveAttention(item, AttentionAction.Discard); runCurrent()
        assertEquals("recovery", session.uncertain?.requestId)
        assertTrue(core.reviewed.isEmpty())
        session.checkUncertain(); runCurrent()
        assertEquals(listOf("origin"), core.reviewed)
        assertEquals(listOf("recovery"), core.submissions)
        assertNull(session.uncertain)
        session.pause()
    }

    @Test fun unknownAcceptIsNotRepeatedByRefreshResumeOrRepeatedButtonPress() = runTest {
        val publication = item.copy(reason = AttentionReason.PreparedPublication, canDiscard = false, canAccept = true)
        val core = Core(publication).apply { accept = { open = emptyList(); error("lost reply") } }
        val session = AppSession(core, backgroundScope)
        session.refreshAttention(); runCurrent()
        session.resolveAttention(publication, AttentionAction.Accept); runCurrent()
        assertEquals(AttentionAction.Accept, session.uncertainAttention?.action)
        session.refreshAttention(); session.resume(); runCurrent()
        session.resolveAttention(publication, AttentionAction.Accept); runCurrent()
        assertEquals(1, core.accepts)
        assertNotNull(session.uncertainAttention)
        session.checkAttentionDecision(); runCurrent()
        assertNull(session.uncertainAttention)
        assertTrue(session.attention.items.isEmpty())
        assertEquals(1, core.accepts)
        session.pause()
    }

    @Test fun pauseDropsPathsAndLateDecisionReplyCannotCloseNewPresentationState() = runTest {
        val publication = item.copy(reason = AttentionReason.PreparedPublication, canDiscard = false, canAccept = true)
        val late = CompletableDeferred<CoreResult<Unit>>()
        val core = Core(publication).apply { accept = { withContext(NonCancellable) { late.await() } } }
        val session = AppSession(core, backgroundScope)
        session.refreshAttention(); runCurrent()
        session.resolveAttention(publication, AttentionAction.Accept); runCurrent(); session.pause()
        assertEquals(AttentionState(), session.attention)
        assertEquals("origin", session.uncertainAttention?.jobId)
        late.complete(CoreResult.Value(Unit)); runCurrent()
        assertEquals(AttentionState(), session.attention)
        assertNotNull(session.uncertainAttention)
        assertEquals(CoreFault.OutcomeUnknown, session.writeFault)
        assertEquals(1, core.accepts)
    }

    @Test fun unsupportedOrStaleActionsNeverDispatchAndFolderLinksOpenTheLogicalParent() = runTest {
        val core = Core(item)
        val session = AppSession(core, backgroundScope)
        session.loadSpaces(); session.refreshAttention(); runCurrent()
        session.resolveAttention(item, AttentionAction.Restore); runCurrent()
        assertEquals(CoreFault.Unsupported, session.writeFault)
        core.open = listOf(item.copy(canDiscard = false))
        session.refreshAttention(); runCurrent()
        session.resolveAttention(item, AttentionAction.Discard); runCurrent()
        assertEquals(CoreFault.Changed, session.writeFault)
        assertTrue(core.submissions.isEmpty()); assertTrue(core.reviewed.isEmpty())
        assertTrue(session.openAttentionFolder(Location("space", "a/b/file.txt"))); runCurrent()
        assertEquals(listOf("", "a", "a/b"), session.files.trail.map { it.location.path })
        session.pause()
    }
}
