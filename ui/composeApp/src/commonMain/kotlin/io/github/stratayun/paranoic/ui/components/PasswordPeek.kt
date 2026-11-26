package io.github.stratayun.paranoic.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.Crossfade
import androidx.compose.foundation.Image
import io.github.stratayun.paranoic.resources.Res
import io.github.stratayun.paranoic.resources.peek_hidden
import io.github.stratayun.paranoic.resources.peek_shown
import org.jetbrains.compose.resources.painterResource
import androidx.compose.foundation.layout.size
import androidx.compose.material3.IconButton
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.actionDescription
import io.github.stratayun.paranoic.ui.state.tr

/** Per-field "show password" flags. A flag is dropped when its field leaves the screen, so reopening a dialog hides it again. */
class PeekStates { internal val visible = mutableStateMapOf<String, Boolean>() }
private val fallbackPeek = PeekStates()
val LocalPeekStates = staticCompositionLocalOf { fallbackPeek }

/** Masked unless this field's eye is open. The keyboard type stays Password either way, so keyboards keep incognito mode. */
@Composable
fun peek(key: String): VisualTransformation =
    if (LocalPeekStates.current.visible[key] == true) VisualTransformation.None else PasswordVisualTransformation()

@Composable
fun PeekToggle(key: String) {
    val states = LocalPeekStates.current
    val shown = states.visible[key] == true
    // Every password field has this toggle: its window is protected from non-tool accessibility services.
    SensitiveWindow()
    DisposableEffect(states, key) { onDispose { states.visible.remove(key) } }
    IconButton(onClick = { states.visible[key] = !shown },
        modifier = Modifier.actionDescription(if (shown) tr("Скрыть пароль", "Hide password") else tr("Показать пароль", "Show password"))) {
        PeekIcon(shown)
    }
}

/** Hidden: a hand covers the eye. Shown: the eye peeks between parted fingers. Crossfade with a small bounce. */
@Composable
fun PeekIcon(shown: Boolean, modifier: Modifier = Modifier) {
    val bounce by animateFloatAsState(if (shown) 1f else 0f, tween(380), label = "peek")
    Crossfade(shown, animationSpec = tween(260), label = "peek-image") { visible ->
        Image(painterResource(if (visible) Res.drawable.peek_shown else Res.drawable.peek_hidden), contentDescription = null,
            modifier = modifier.size(28.dp).graphicsLayer {
                val s = 1f + 0.08f * kotlin.math.sin(bounce * kotlin.math.PI.toFloat())
                scaleX = s; scaleY = s
            })
    }
}
