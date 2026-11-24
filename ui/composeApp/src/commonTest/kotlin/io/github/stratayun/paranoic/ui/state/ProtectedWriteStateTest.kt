package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.CoreFault
import kotlin.test.*

class ProtectedWriteStateTest {
    @Test fun aRunningWriteCannotBeSubmittedTwiceOrAcknowledged() {
        val running = assertNotNull(ProtectedWriteState().begin())
        assertEquals(ProtectedWritePhase.Running, running.phase)
        assertNull(running.begin())
        assertSame(running, running.acknowledge())
    }
    @Test fun confirmedCompletionAllowsANewUniqueTicket() {
        val first = assertNotNull(ProtectedWriteState().begin())
        val ready = first.finish(first.ticket, uncertain = false)
        val second = assertNotNull(ready.begin())
        assertTrue(second.ticket > first.ticket)
        assertSame(second, second.finish(first.ticket, uncertain = false))
    }
    @Test fun unknownCompletionNeedsExplicitAcknowledgement() {
        val running = assertNotNull(ProtectedWriteState().begin())
        val unknown = running.finish(running.ticket, uncertain = true)
        assertEquals(ProtectedWritePhase.Unknown, unknown.phase)
        assertNull(unknown.begin())
        assertNotNull(unknown.acknowledge().begin())
    }
    @Test fun aLateSuccessfulReplyCannotEraseInterruptionUncertainty() {
        val running = assertNotNull(ProtectedWriteState().begin())
        val interrupted = running.interrupted()
        assertEquals(ProtectedWritePhase.Unknown, interrupted.phase)
        assertSame(interrupted, interrupted.finish(running.ticket, uncertain = false))
        assertSame(interrupted, interrupted.interrupted())
    }
    @Test fun anOldReplyCannotCompleteAWriteStartedAfterReconciliation() {
        val first = assertNotNull(ProtectedWriteState().begin())
        val second = assertNotNull(first.interrupted().acknowledge().begin())
        assertSame(second, second.finish(first.ticket, uncertain = true))
        assertEquals(ProtectedWritePhase.Running, second.phase)
    }
    @Test fun ticketExhaustionIsFailClosed() {
        assertNull(ProtectedWriteState(ticket = Long.MAX_VALUE).begin())
        val ready = ProtectedWriteState()
        assertSame(ready, ready.interrupted())
    }
    @Test fun unknownOutcomeClassificationDoesNotTurnKnownRefusalsIntoRetries() {
        val uncertain = setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable, CoreFault.Disconnected, CoreFault.Cancelled)
        CoreFault.entries.forEach { assertEquals(it in uncertain, it.hasUnknownWriteOutcome(), it.name) }
    }
}
