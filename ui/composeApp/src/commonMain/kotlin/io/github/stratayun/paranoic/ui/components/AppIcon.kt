package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.theme.LocalThemeTokens

enum class AppSymbol { Home, Settings, Back, Folder, Document, Cloud, Security, Vault, Recovery, Search, Analytics, Trash, Sync, Passwords, Photo, Video, Audio, Archive, Favorite, Recent, More }

/** Decorative vectors: the surrounding labelled control owns accessibility. */
@Composable
fun AppIcon(symbol: AppSymbol, modifier: Modifier = Modifier) {
    val color = LocalContentColor.current
    val width = LocalThemeTokens.current.iconStroke
    Canvas(modifier.size(24.dp)) {
        val u = size.width / 24
        val stroke = Stroke(width.toPx(), cap = StrokeCap.Round)
        when (symbol) {
            AppSymbol.Document -> {
                val path = Path().apply {
                    moveTo(6 * u, 3 * u); lineTo(14 * u, 3 * u); lineTo(19 * u, 8 * u)
                    lineTo(19 * u, 21 * u); lineTo(6 * u, 21 * u); close()
                    moveTo(14 * u, 3 * u); lineTo(14 * u, 8 * u); lineTo(19 * u, 8 * u)
                    moveTo(9 * u, 12 * u); lineTo(16 * u, 12 * u)
                    moveTo(9 * u, 16 * u); lineTo(16 * u, 16 * u)
                }
                drawPath(path, color, style = stroke)
            }
            AppSymbol.Home -> {
                val path = Path().apply {
                    moveTo(3 * u, 11 * u); lineTo(12 * u, 3 * u); lineTo(21 * u, 11 * u)
                    moveTo(5 * u, 10 * u); lineTo(5 * u, 21 * u); lineTo(19 * u, 21 * u); lineTo(19 * u, 10 * u)
                }
                drawPath(path, color, style = stroke)
            }
            AppSymbol.Settings -> {
                for ((y, x) in listOf(6f to 8f, 12f to 16f, 18f to 10f)) {
                    drawLine(color, Offset(3 * u, y * u), Offset(21 * u, y * u), width.toPx(), StrokeCap.Round)
                    drawCircle(color, 2.4f * u, Offset(x * u, y * u))
                }
            }
            AppSymbol.Back -> {
                val path = Path().apply { moveTo(15 * u, 5 * u); lineTo(8 * u, 12 * u); lineTo(15 * u, 19 * u) }
                drawPath(path, color, style = stroke)
            }
            AppSymbol.Folder -> {
                drawRoundRect(color, Offset(3 * u, 7 * u), Size(18 * u, 13 * u), CornerRadius(2 * u), style = stroke)
                drawLine(color, Offset(4 * u, 4 * u), Offset(10 * u, 4 * u), width.toPx(), StrokeCap.Round)
            }
            else -> drawCircle(color, 8 * u, style = stroke)
        }
    }
}

