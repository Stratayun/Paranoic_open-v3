package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.Canvas
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

/** Accent card for a primary destination (reference: the lilac "Сейф" card). */
@Composable
fun AccentCard(title: String, subtitle: String, symbol: AppSymbol, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(32.dp), color = scheme.tertiaryContainer, contentColor = scheme.onTertiaryContainer) {
        Row(Modifier.clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 20.dp, vertical = 18.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            ShapedIcon(symbol, ExpressiveShapes.Cookie9, scheme.onTertiaryContainer.copy(alpha = 0.10f), scheme.onTertiaryContainer, size = 56.dp)
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleLarge)
                Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = scheme.onTertiaryContainer.copy(alpha = 0.8f))
            }
            AppIcon(AppSymbol.Forward)
        }
    }
}

/** Wavy determinate progress (M3 Expressive): wave up to [progress], flat track after. */
@Composable
fun WavyProgress(progress: Float, modifier: Modifier = Modifier, color: Color = MaterialTheme.colorScheme.primary,
    track: Color = MaterialTheme.colorScheme.surfaceContainerHighest, thickness: Dp = 6.dp, wavelength: Dp = 28.dp) {
    val value = progress.coerceIn(0f, 1f)
    Canvas(modifier.fillMaxWidth().height(thickness * 3)) {
        val stroke = thickness.toPx()
        val cy = size.height / 2
        val amplitude = (size.height - stroke) / 2 * 0.6f
        val end = size.width * value
        val gap = stroke * 1.5f
        if (end + gap < size.width) drawLine(track, Offset(end + gap, cy), Offset(size.width - stroke / 2, cy), stroke, StrokeCap.Round)
        if (end > stroke) {
            val path = Path()
            val k = 2 * PI.toFloat() / wavelength.toPx()
            var x = stroke / 2
            path.moveTo(x, cy)
            while (x <= end) { path.lineTo(x, cy + amplitude * sin(k * x)); x += 2f }
            drawPath(path, color, style = Stroke(stroke, cap = StrokeCap.Round))
        }
    }
}
