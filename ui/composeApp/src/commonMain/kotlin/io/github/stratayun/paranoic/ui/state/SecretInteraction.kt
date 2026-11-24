package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput

internal val LocalSecretActivity = staticCompositionLocalOf<() -> Unit> { {} }

/** Dialog windows inherit the same activity callback as the vault screen. */
@Composable
internal fun Modifier.trackSecretActivity(): Modifier {
    val latest by rememberUpdatedState(LocalSecretActivity.current)
    return onPreviewKeyEvent { latest(); false }.pointerInput(Unit) {
        awaitPointerEventScope { while (true) {
            val event = awaitPointerEvent(PointerEventPass.Initial)
            if (event.changes.any { it.pressed }) latest()
        } }
    }
}
