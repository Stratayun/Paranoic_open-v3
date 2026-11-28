package io.github.stratayun.paranoic.ui.components

import androidx.compose.animation.core.*
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.ui.state.VolumeUsage
import kotlin.math.roundToLong
import io.github.stratayun.paranoic.ui.state.tr

/**
 * Storage card after the owner's animation (27.11): outlined drive icon, title, "X занято из Y",
 * large percentage, a bar that fills to the used share with a moving highlight, and a legend.
 * Sizes are decimal GB, as Android's own storage screen shows them.
 */
@Composable
fun StorageUsageCard(title: String, usage: VolumeUsage, symbol: AppSymbol = AppSymbol.Storage,
    modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    val scheme = MaterialTheme.colorScheme
    val used = (usage.totalBytes - usage.freeBytes).coerceAtLeast(0)
    val fraction = if (usage.totalBytes > 0) (used.toDouble() / usage.totalBytes).toFloat().coerceIn(0f, 1f) else 0f
    val percent = decimal(fraction * 100.0) + "%"
    val fill = remember { Animatable(0f) }
    LaunchedEffect(fraction) { fill.animateTo(fraction, tween(1100, easing = FastOutSlowInEasing)) }
    val shimmer = rememberInfiniteTransition(label = "storage").animateFloat(-0.3f, 1.3f,
        infiniteRepeatable(tween(2200, easing = LinearEasing), RepeatMode.Restart), label = "shimmer")
    val danger = fraction > 0.9f
    val fillStart = if (danger) Color(0xFFB8435A) else Color(0xFF4F6FC4)
    val fillEnd = if (danger) Color(0xFFFF9AA8) else Color(0xFFB4CCFF)
    Surface(modifier.fillMaxWidth().semantics { contentDescription = "$title, $percent" }, shape = RoundedCornerShape(24.dp),
        color = Color.Transparent, border = BorderStroke(1.dp, scheme.outline.copy(alpha = 0.55f))) {
        Column(Modifier.background(Brush.verticalGradient(listOf(scheme.surfaceContainerHigh, scheme.surfaceContainer)))
            .let { if (onClick != null) it.clickable(role = Role.Button, onClick = onClick) else it }
            .padding(horizontal = 18.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(46.dp).clip(RoundedCornerShape(14.dp)).background(scheme.surfaceContainerLowest)
                    .border(1.dp, scheme.outline.copy(alpha = 0.6f), RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
                    if (symbol == AppSymbol.Storage) DriveGlyph(scheme.onSurface)
                    else Icon(symbol.vector(), null, Modifier.size(24.dp), tint = scheme.onSurface)
                }
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.titleMedium.copy(fontSize = 18.sp), fontWeight = FontWeight.SemiBold,
                        maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(storageGb(used) + tr(" занято из ", " used of ") + storageGb(usage.totalBytes), style = MaterialTheme.typography.bodySmall,
                        color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Spacer(Modifier.width(8.dp))
                Text(percent, style = MaterialTheme.typography.titleLarge.copy(fontSize = 24.sp), fontWeight = FontWeight.SemiBold, maxLines = 1)
            }
            Canvas(Modifier.fillMaxWidth().height(14.dp)) {
                val r = CornerRadius(size.height / 2, size.height / 2)
                drawRoundRect(scheme.surfaceContainerLowest, cornerRadius = r)
                drawRoundRect(scheme.outline.copy(alpha = 0.6f), cornerRadius = r, style = androidx.compose.ui.graphics.drawscope.Stroke(1.dp.toPx()))
                val w = size.width * fill.value
                if (w > 0f) {
                    drawRoundRect(Brush.horizontalGradient(listOf(fillStart, fillEnd), 0f, w.coerceAtLeast(1f)),
                        size = Size(w, size.height), cornerRadius = r)
                    // Moving highlight inside the filled part.
                    val band = size.width * 0.18f
                    val x = shimmer.value * size.width
                    val left = (x - band).coerceIn(0f, w); val right = (x + band).coerceIn(0f, w)
                    if (right > left) drawRoundRect(Brush.horizontalGradient(listOf(Color.Transparent, Color.White.copy(alpha = 0.35f), Color.Transparent), left, right),
                        topLeft = Offset(left, 0f), size = Size(right - left, size.height), cornerRadius = r)
                }
            }
            HorizontalDivider(color = scheme.outlineVariant)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Legend(fillEnd, tr("Занято", "Used"), storageGb(used))
                Spacer(Modifier.weight(1f))
                Legend(scheme.outline, tr("Свободно", "Free"), storageGb(usage.freeBytes))
            }
        }
    }
}

/** Outlined drive, as in the owner's animation. */
@Composable
private fun DriveGlyph(color: Color) {
    Canvas(Modifier.size(26.dp, 22.dp)) {
        val stroke = 1.8.dp.toPx()
        val body = Size(size.width - stroke, size.height * 0.62f)
        val top = size.height - body.height - stroke / 2
        drawRoundRect(color, topLeft = Offset(stroke / 2, top), size = body, cornerRadius = CornerRadius(4.dp.toPx()),
            style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        // Upper slanted housing.
        val path = androidx.compose.ui.graphics.Path().apply {
            moveTo(stroke / 2 + 2.dp.toPx(), top); lineTo(5.dp.toPx(), stroke); lineTo(size.width - 5.dp.toPx(), stroke)
            lineTo(size.width - stroke / 2 - 2.dp.toPx(), top)
        }
        drawPath(path, color, style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        val midY = top + body.height * 0.45f
        drawLine(color, Offset(stroke / 2, midY), Offset(size.width - stroke / 2, midY), stroke * 0.8f)
        drawCircle(color, 1.6.dp.toPx(), Offset(size.width - 5.dp.toPx(), top + body.height * 0.73f))
    }
}

@Composable
private fun Legend(dot: Color, label: String, value: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        Spacer(Modifier.width(8.dp))
        Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(12.dp))
        Text(value, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.Bold)
    }
}

/** Decimal GB with one digit ("118,4 ГБ"); whole numbers without the decimal ("256 ГБ"). */
@Composable
internal fun storageGb(bytes: Long): String {
    val tenths = (bytes / 1e8).roundToLong()
    val text = if (tenths % 10 == 0L) "${tenths / 10}" else "${tenths / 10}${tr(",", ".")}${tenths % 10}"
    return text + " " + tr("ГБ", "GB")
}

@Composable
private fun decimal(value: Double): String {
    val tenths = (value * 10).roundToLong()
    return "${tenths / 10}${tr(",", ".")}${tenths % 10}"
}
