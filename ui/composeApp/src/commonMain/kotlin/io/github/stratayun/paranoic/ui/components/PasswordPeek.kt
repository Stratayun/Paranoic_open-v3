package io.github.stratayun.paranoic.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
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
    DisposableEffect(states, key) { onDispose { states.visible.remove(key) } }
    IconButton(onClick = { states.visible[key] = !shown },
        modifier = Modifier.actionDescription(if (shown) tr("Скрыть пароль", "Hide password") else tr("Показать пароль", "Show password"))) {
        PeekIcon(shown)
    }
}

/**
 * Hidden: a hand covers the eye. Shown: the fingers part and the eye squints
 * through the gaps. Drawn offscreen so the gaps between fingers are real cut-outs.
 */
@Composable
fun PeekIcon(shown: Boolean, modifier: Modifier = Modifier) {
    val t by animateFloatAsState(if (shown) 1f else 0f, tween(380), label = "peek")
    val color = LocalContentColor.current
    Canvas(modifier.size(24.dp).graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)) {
        val u = size.width / 24f
        drawEye(color, u, squint = t)
        drawHand(color, u, spread = t)
    }
}

private fun lerp(a: Float, b: Float, t: Float) = a + (b - a) * t

private fun DrawScope.drawEye(color: Color, u: Float, squint: Float) {
    val cy = 10.5f * u
    // Open eye when covered (unseen anyway), narrowed to a squint when peeking.
    val h = lerp(4.6f, 2.2f, squint) * u
    val eye = Path().apply {
        moveTo(3.5f * u, cy)
        quadraticTo(12f * u, cy - 2 * h, 20.5f * u, cy)
        quadraticTo(12f * u, cy + 2 * h, 3.5f * u, cy)
        close()
    }
    drawPath(eye, color, style = Stroke(1.7f * u))
    drawCircle(color, lerp(2.6f, 1.6f, squint) * u, Offset(12f * u, cy))
}

private fun DrawScope.drawHand(color: Color, u: Float, spread: Float) {
    // Peeking: the hand drops and shrinks, the middle fingers part wide around the pupil.
    val scale = lerp(1f, 0.72f, spread)
    val top = lerp(4.5f, 9.5f, spread) * u
    val finger = lerp(2.6f, 2.2f, spread) * u
    val gaps = floatArrayOf(lerp(0.4f, 1.2f, spread), lerp(0.4f, 4.8f, spread), lerp(0.4f, 1.2f, spread))
    val total = 4 * finger + gaps.sum() * u
    var x = 12f * u - total / 2
    val heights = floatArrayOf(9f, 10.5f, 10.5f, 9f)
    val margin = 0.55f * u
    for (i in 0..3) {
        val h = heights[i] * scale * u
        val y = top + (10.5f - heights[i]) * scale * u
        // Cut a thin outline first so overlapping eye strokes do not merge with the fingers.
        drawRoundRect(Color.Black, Offset(x - margin, y - margin), Size(finger + 2 * margin, h + 2 * margin),
            CornerRadius(finger), blendMode = BlendMode.Clear)
        drawRoundRect(color, Offset(x, y), Size(finger, h), CornerRadius(finger / 2))
        if (i < 3) x += finger + gaps[i] * u
    }
    val left = 12f * u - total / 2
    val palmTop = top + 7.5f * scale * u
    val palmH = lerp(6f, 5f, spread) * u
    drawRoundRect(Color.Black, Offset(left - margin, palmTop - margin), Size(total + 2 * margin, palmH + 2 * margin),
        CornerRadius(3f * u), blendMode = BlendMode.Clear)
    drawRoundRect(color, Offset(left, palmTop), Size(total, palmH), CornerRadius(2.6f * u))
}
