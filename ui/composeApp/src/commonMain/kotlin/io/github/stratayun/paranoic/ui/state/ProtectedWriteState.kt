package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

enum class ProtectedWritePhase { Ready, Running, Unknown }

/** Non-secret, session-only barrier for direct protected writes without a request receipt.
 * A screen disappearing cannot turn an unconfirmed write into a safe retry. */
data class ProtectedWriteState(val ticket: Long = 0, val phase: ProtectedWritePhase = ProtectedWritePhase.Ready) {
    fun begin(): ProtectedWriteState? = if (phase == ProtectedWritePhase.Ready && ticket < Long.MAX_VALUE)
        copy(ticket = ticket + 1, phase = ProtectedWritePhase.Running) else null
    fun finish(receivedTicket: Long, uncertain: Boolean): ProtectedWriteState =
        if (receivedTicket != ticket || phase != ProtectedWritePhase.Running) this
        else copy(phase = if (uncertain) ProtectedWritePhase.Unknown else ProtectedWritePhase.Ready)
    fun interrupted(): ProtectedWriteState = if (phase == ProtectedWritePhase.Running)
        copy(phase = ProtectedWritePhase.Unknown) else this
    fun acknowledge(): ProtectedWriteState = if (phase == ProtectedWritePhase.Unknown)
        copy(phase = ProtectedWritePhase.Ready) else this
}

fun CoreFault.hasUnknownWriteOutcome() = this in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
    CoreFault.Disconnected, CoreFault.Cancelled)
