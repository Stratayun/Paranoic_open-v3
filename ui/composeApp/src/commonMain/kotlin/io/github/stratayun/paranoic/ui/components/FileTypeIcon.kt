package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.resources.*
import org.jetbrains.compose.resources.DrawableResource

private val images = setOf("png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "avif", "svg")
private val videos = setOf("mp4", "mkv", "webm", "mov", "avi", "3gp", "m4v")
private val audio = setOf("mp3", "flac", "ogg", "opus", "m4a", "wav", "aac", "wma")
private val archives = setOf("zip", "7z", "rar", "tar", "gz", "xz", "zst", "bz2")
private val encrypted = setOf("age", "c9r", "pnc")

/** Neon-glass type icon for a file row, with a small extension tag (PDF, ZIP, MP3…). */
fun fileTypeIcon(name: String, folder: Boolean): DrawableResource {
    if (folder) return Res.drawable.ic_folder
    return when (name.substringAfterLast('.', "").lowercase()) {
        in images -> Res.drawable.ic_images
        in videos -> Res.drawable.ic_video
        in audio -> Res.drawable.ic_music
        // No archive/APK artwork in the set yet: the box of tiles stands in.
        in archives -> Res.drawable.ic_tiles
        in encrypted -> Res.drawable.ic_lock
        "apk" -> Res.drawable.ic_tiles
        else -> Res.drawable.ic_documents
    }
}

@Composable
fun FileTypeIcon(name: String, folder: Boolean, modifier: Modifier = Modifier, size: Dp = 48.dp) {
    val ext = if (folder) "" else name.substringAfterLast('.', "").take(4).uppercase()
    Box(modifier.size(size)) {
        BrandIcon(fileTypeIcon(name, folder), size = size)
        if (ext.isNotEmpty() && size >= 40.dp) Text(ext, Modifier.align(Alignment.BottomEnd)
            .background(Color(0xE6101218), RoundedCornerShape(4.dp)).padding(horizontal = 3.dp),
            color = Color.White, fontSize = 7.sp, lineHeight = 9.sp, fontWeight = FontWeight.Bold, maxLines = 1, style = MaterialTheme.typography.labelSmall)
    }
}
