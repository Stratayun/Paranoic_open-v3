package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*

/** Shared by the editor and every app navigation route. UI thread only. */
class DocumentExitGuard {
    var dirty by mutableStateOf(false)
    var busy by mutableStateOf(false)
    var uncertain by mutableStateOf(false)
    var pending by mutableStateOf<(() -> Unit)?>(null)
        private set

    fun request(action: () -> Unit) {
        if (busy) return
        if (dirty || uncertain) pending = action else action()
    }
    fun cancel() { pending = null }
    fun discard() {
        if (busy) return
        val action = pending
        pending = null
        dirty = false
        uncertain = false
        action?.invoke()
    }
}
