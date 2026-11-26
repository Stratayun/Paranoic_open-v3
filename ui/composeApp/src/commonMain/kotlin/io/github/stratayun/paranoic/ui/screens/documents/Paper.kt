package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * One look for every document (owner 26.11): Word, Excel and Markdown are shown on a light
 * "sheet of paper" over the dark desk, like the rendered PDF pages.
 */
internal object Paper {
    val sheet = Color(0xFFFCFCFA)
    val ink = Color(0xFF1C1F2A)
    val muted = Color(0xFF5E6474)
    val accent = Color(0xFF2E3D9A)
    val rule = Color(0xFFD6DAE4)
    val header = Color(0xFFEDF0F7)
    val code = Color(0xFFF1F2F5)
    val quote = Color(0xFF8C96C8)

    val body = TextStyle(color = ink, fontSize = 15.sp, lineHeight = 23.sp)
    val small = TextStyle(color = muted, fontSize = 12.sp, lineHeight = 16.sp)
    fun heading(level: Int) = TextStyle(color = accent, fontWeight = FontWeight.SemiBold, fontSize = when (level) {
        0, 1 -> 24.sp; 2 -> 20.sp; 3 -> 18.sp; else -> 16.sp
    }, lineHeight = when (level) { 0, 1 -> 30.sp; 2 -> 26.sp; 3 -> 24.sp; else -> 22.sp })
}

/** Light colours inside the page, so selection handles, dividers and dialogs launched from it stay readable. */
private val PaperScheme = lightColorScheme(primary = Paper.accent, onPrimary = Color.White, surface = Paper.sheet,
    onSurface = Paper.ink, onSurfaceVariant = Paper.muted, surfaceVariant = Paper.header, outline = Paper.rule)

/** The page: centred, at most a book-page wide on large windows, shadow and thin edge like a PDF page. */
@Composable
internal fun PaperPage(modifier: Modifier = Modifier, padding: PaddingValues = PaddingValues(horizontal = 20.dp, vertical = 18.dp),
    content: @Composable BoxScope.() -> Unit) {
    Box(modifier.fillMaxSize().padding(horizontal = 8.dp, vertical = 8.dp), contentAlignment = Alignment.TopCenter) {
        MaterialTheme(colorScheme = PaperScheme, typography = MaterialTheme.typography, shapes = MaterialTheme.shapes) {
            Surface(Modifier.widthIn(max = 860.dp).fillMaxSize(), shape = RoundedCornerShape(6.dp), color = Paper.sheet,
                contentColor = Paper.ink, shadowElevation = 6.dp, border = BorderStroke(1.dp, Paper.rule)) {
                Box(Modifier.fillMaxSize().padding(padding), content = content)
            }
        }
    }
}
