package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

data class AttentionState(val items: List<AttentionItem> = emptyList(), val loaded: Boolean = false,
    val loading: Boolean = false, val fault: CoreFault? = null)

enum class AttentionAction { Restore, Discard, Accept, Reviewed }
/** No file names or paths: survives pause only to prevent silently repeating a decision. */
data class UncertainAttention(val jobId: String, val action: AttentionAction)
