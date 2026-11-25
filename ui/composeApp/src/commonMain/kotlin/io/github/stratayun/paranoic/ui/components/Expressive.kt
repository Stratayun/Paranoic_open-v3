package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.ui.graphics.Brush
import androidx.compose.animation.core.*
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.GenericShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.sectionHeading
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin

/**
 * Material 3 Expressive style shapes, drawn locally: Compose Multiplatform
 * 1.9 does not expose MaterialShapes. A polar curve r(θ) = 1 + a·cos(nθ)
 * gives "cookie", "sunny" and rounded-polygon outlines with smooth lobes.
 */
object ExpressiveShapes {
    val Circle: Shape = RoundedCornerShape(50)
    val Pill: Shape = RoundedCornerShape(50)
    val Cookie4: Shape = polar(4, 0.10f)
    val Cookie6: Shape = polar(6, 0.08f)
    val Cookie9: Shape = polar(9, 0.06f)
    val Sunny: Shape = polar(8, 0.11f)
    val Flower: Shape = polar(12, 0.07f)
    val Clover: Shape = polar(4, 0.22f)
    val Triangle: Shape = polar(3, 0.16f, rotation = -PI / 2)

    /** Rotates through shapes so neighbouring icons differ, like the reference drawer. */
    val gallery: List<Shape> = listOf(Cookie9, Sunny, Cookie6, Flower, Clover, Cookie4, Triangle)

    fun polar(lobes: Int, depth: Float, rotation: Double = 0.0, steps: Int = 180): Shape = GenericShape { size, _ ->
        val cx = size.width / 2f
        val cy = size.height / 2f
        val radius = min(cx, cy) / (1f + depth)
        for (i in 0..steps) {
            val t = 2 * PI * i / steps
            val r = radius * (1f + depth * cos(lobes * (t - rotation)).toFloat())
            val x = cx + r * cos(t).toFloat()
            val y = cy + r * sin(t).toFloat()
            if (i == 0) moveTo(x, y) else lineTo(x, y)
        }
        close()
    }
}

/** Icon on a tonal shaped container (reference: drawer places and tools). */
@Composable
fun ShapedIcon(symbol: AppSymbol, shape: Shape, container: Color, content: Color, modifier: Modifier = Modifier, size: Dp = 48.dp) {
    Box(modifier.size(size).clip(shape).background(container), contentAlignment = Alignment.Center) {
        CompositionLocalProvider(LocalContentColor provides content) {
            AppIcon(symbol, Modifier.size(size * 0.5f))
        }
    }
}

/** Position of a row inside a grouped list: outer corners are large, inner ones small. */
enum class GroupPosition { Single, First, Middle, Last }

fun groupPosition(index: Int, count: Int): GroupPosition = when {
    count <= 1 -> GroupPosition.Single
    index == 0 -> GroupPosition.First
    index == count - 1 -> GroupPosition.Last
    else -> GroupPosition.Middle
}

fun groupShape(position: GroupPosition, outer: Dp = 28.dp, inner: Dp = 6.dp): Shape = when (position) {
    GroupPosition.Single -> RoundedCornerShape(outer)
    GroupPosition.First -> RoundedCornerShape(outer, outer, inner, inner)
    GroupPosition.Middle -> RoundedCornerShape(inner)
    GroupPosition.Last -> RoundedCornerShape(inner, inner, outer, outer)
}

/**
 * One row of a grouped list. Whole row is the click target; secondary actions
 * live in [trailing] (e.g. an overflow menu), never as a row of text buttons.
 */
@Composable
fun GroupedRow(
    title: String,
    position: GroupPosition,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    enabled: Boolean = true,
    container: Color = MaterialTheme.colorScheme.surfaceContainerHigh,
    onClick: (() -> Unit)? = null,
) {
    Surface(modifier.fillMaxWidth(), shape = groupShape(position), color = container) {
        Row(
            Modifier.then(if (onClick != null) Modifier.clickable(enabled = enabled, role = Role.Button, onClick = onClick) else Modifier)
                .heightIn(min = 64.dp).padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            leading?.invoke()
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            trailing?.invoke()
        }
    }
}

/** Grouped list: rows separated by a 2dp gap, outer corners rounded. */
@Composable
fun <T> GroupedList(items: List<T>, modifier: Modifier = Modifier, row: @Composable (item: T, position: GroupPosition) -> Unit) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        items.forEachIndexed { index, item -> row(item, groupPosition(index, items.size)) }
    }
}

/** Section label in the primary colour, as in the reference drawer ("Места", "Система"). */
@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier) {
    Text(text, modifier.sectionHeading().padding(start = 12.dp, top = 20.dp, bottom = 8.dp),
        style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
}

/**
 * Storage bar (owner's reference): a thick glass capsule with a dark rim; the filled part is a
 * purple→blue gradient with soft waves flowing inside it and a glowing orb at the fill edge.
 */
@Composable
fun WavyProgress(progress: Float, modifier: Modifier = Modifier, color: Color = MaterialTheme.colorScheme.primary,
    track: Color = MaterialTheme.colorScheme.surfaceContainerHighest, thickness: Dp = 22.dp, wavelength: Dp = 90.dp, animated: Boolean = true) {
    val value = progress.coerceIn(0f, 1f)
    val shift = if (animated) {
        val transition = rememberInfiniteTransition(label = "wave")
        transition.animateFloat(0f, 1f, infiniteRepeatable(tween(3200, easing = LinearEasing)), label = "phase").value
    } else 0f
    val pulse = if (animated) {
        val transition = rememberInfiniteTransition(label = "orb")
        transition.animateFloat(0.75f, 1f, infiniteRepeatable(tween(1400), RepeatMode.Reverse), label = "glow").value
    } else 1f
    val purple = Color(0xFF8E6CF5)
    val blue = Color(0xFF2F7BFF)
    val cyan = Color(0xFF3FD5FF)
    Canvas(modifier.fillMaxWidth().height(thickness + 12.dp)) {
        val h = thickness.toPx()
        val top = (size.height - h) / 2
        val r = CornerRadius(h / 2)
        val rim = 3.dp.toPx()
        // Capsule with a dark rim and a dim glass track.
        drawRoundRect(Color(0xFF0B1230), Offset(0f, top - rim), Size(size.width, h + 2 * rim), CornerRadius(h / 2 + rim))
        drawRoundRect(Color(0xFF1A2342), Offset(rim, top), Size(size.width - 2 * rim, h), r)
        val fillEnd = rim + (size.width - 2 * rim) * value
        if (fillEnd > rim + h / 2) {
            val fill = Path().apply { addRoundRect(RoundRect(rim, top, fillEnd, top + h, r)) }
            clipPath(fill) {
                drawRect(Brush.horizontalGradient(listOf(purple, blue, cyan), rim, fillEnd), Offset(rim, top), Size(fillEnd - rim, h))
                // Two translucent waves drift inside the fill.
                val k = 2 * PI.toFloat() / wavelength.toPx()
                val phase = shift * 2 * PI.toFloat()
                for ((alpha, offset, amp) in listOf(Triple(0.28f, 0f, 0.22f), Triple(0.18f, 1.7f, 0.3f))) {
                    val wave = Path()
                    var x = rim
                    wave.moveTo(x, top + h)
                    while (x <= fillEnd) { wave.lineTo(x, top + h * 0.55f + h * amp * sin(k * x - phase + offset)); x += 3f }
                    wave.lineTo(fillEnd, top + h); wave.close()
                    drawPath(wave, Color.White.copy(alpha = alpha))
                }
                // Gloss on the upper half.
                drawRect(Brush.verticalGradient(listOf(Color.White.copy(alpha = 0.22f), Color.Transparent), top, top + h * 0.5f),
                    Offset(rim, top), Size(fillEnd - rim, h * 0.5f))
            }
            val orb = Offset(fillEnd - h * 0.45f, top + h / 2)
            drawCircle(Brush.radialGradient(listOf(cyan.copy(alpha = 0.65f * pulse), Color.Transparent), orb, h * 1.1f), h * 1.1f, orb)
            drawCircle(Color.White, h * 0.32f, orb)
        }
    }
}

/** Icons from the Paranoic icon set (glass blobs). Decorative. */
@Composable
fun BrandIcon(icon: org.jetbrains.compose.resources.DrawableResource, modifier: Modifier = Modifier, size: Dp = 48.dp) {
    androidx.compose.foundation.Image(org.jetbrains.compose.resources.painterResource(icon), contentDescription = null, modifier = modifier.size(size))
}
