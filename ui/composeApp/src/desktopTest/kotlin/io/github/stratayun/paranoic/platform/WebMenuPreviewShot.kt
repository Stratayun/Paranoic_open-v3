package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.ImageComposeScene
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.AppIcon
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.components.BrandIcon
import io.github.stratayun.paranoic.ui.state.AppearanceSettings
import io.github.stratayun.paranoic.ui.theme.ParanoicTheme
import org.jetbrains.compose.resources.DrawableResource
import org.jetbrains.skia.EncodedImageFormat
import java.io.File
import kotlin.math.cos
import kotlin.math.sin
import kotlin.test.Test

/** Concept preview of the "web" navigation theme (owner 26.11). Renders only with PARANOIC_PREVIEW_OUT. */
class WebMenuPreviewShot {
    private data class Node(val label: String, val icon: DrawableResource, val ring: Int, val angle: Float)

    private val nodes = listOf(
        Node("Файлы", Res.drawable.ic_folder, 1, 183f), Node("Поиск", Res.drawable.ic_search, 1, 210f),
        Node("Сейф", Res.drawable.ic_safe, 1, 238f), Node("Крипто-\nпапки", Res.drawable.ic_crypto_folder, 1, 266f),
        Node("Загрузки", Res.drawable.ic_downloads, 2, 182f), Node("Документы", Res.drawable.ic_documents, 2, 199f),
        Node("Картинки", Res.drawable.ic_images, 2, 216f), Node("Видео", Res.drawable.ic_video, 2, 233f),
        Node("Музыка", Res.drawable.ic_music, 2, 250f), Node("Корзина", Res.drawable.ic_trash_full, 2, 267f),
        Node("Очистка", Res.drawable.ic_cleanup, 3, 188f), Node("Свой\nсервер", Res.drawable.ic_cloud, 3, 208f),
        Node("Синхро-\nнизация", Res.drawable.ic_sync, 3, 228f), Node("Диагно-\nстика", Res.drawable.ic_diagnostics, 3, 248f),
        Node("Настройки", Res.drawable.ic_settings, 3, 268f),
    )
    private val radius = mapOf(1 to 135f, 2 to 250f, 3 to 360f)
    private val kx = 0.88f; private val ky = 1.3f
    private val size = mapOf(1 to 58.dp, 2 to 48.dp, 3 to 44.dp)

    @Test fun web() {
        val out = System.getenv("PARANOIC_PREVIEW_OUT") ?: return
        for (open in listOf(false, true)) {
            val scene = ImageComposeScene(1080, 2340, Density(2.625f)) { ParanoicTheme(AppearanceSettings()) { Phone(open) } }
            val image = scene.render()
            File(out.replace(".png", if (open) "-open.png" else "-closed.png")).writeBytes(image.encodeToData(EncodedImageFormat.PNG)!!.bytes)
            scene.close()
        }
    }

    @Composable
    private fun Phone(open: Boolean) {
        Box(Modifier.fillMaxSize().background(Color(0xFF070B24))) {
            AppBehind(Modifier.fillMaxSize().then(if (open) Modifier.blur(10.dp) else Modifier))
            if (open) Box(Modifier.fillMaxSize().background(Brush.radialGradient(listOf(Color(0xCC0A0F35), Color(0x99070B24)),
                center = Offset(1000f, 2150f), radius = 2200f)))
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val cx = maxWidth.value - 46f; val cy = maxHeight.value - 70f
                fun pos(n: Node) = Offset(cx + kx * radius[n.ring]!! * cos(Math.toRadians(n.angle.toDouble())).toFloat(),
                    cy + ky * radius[n.ring]!! * sin(Math.toRadians(n.angle.toDouble())).toFloat())
                if (open) {
                    Canvas(Modifier.fillMaxSize()) {
                        val d = density
                        val center = Offset(cx * d, cy * d)
                        // Faint rings, then threads: centre → ring 1, each node → nearest of the next ring, neighbours on a ring.
                        for (r in radius.values) drawOval(Color(0x228A7CFF), Offset(center.x - kx * r * d, center.y - ky * r * d), androidx.compose.ui.geometry.Size(2 * kx * r * d, 2 * ky * r * d), style = Stroke(1.2f * d))
                        val byRing = nodes.groupBy { it.ring }
                        fun line(a: Offset, b: Offset, alpha: Float) = drawLine(Brush.linearGradient(listOf(Color(0xFF7C8CFF).copy(alpha = alpha),
                            Color(0xFF6FD8FF).copy(alpha = alpha * 0.7f)), a, b), a, b, 1.6f * d)
                        byRing[1]!!.forEach { line(center, pos(it) * d, 0.75f) }
                        for (ring in 1..2) byRing[ring]!!.forEach { n ->
                            byRing[ring + 1]!!.sortedBy { kotlin.math.abs(it.angle - n.angle) }.take(2).forEach { line(pos(n) * d, pos(it) * d, 0.45f) }
                        }
                        byRing.values.forEach { list -> list.sortedBy { it.angle }.zipWithNext().forEach { (a, b) -> line(pos(a) * d, pos(b) * d, 0.35f) } }
                        nodes.forEach { drawCircle(Color(0xFF9DA8FF), 2.5f * d, pos(it) * d) }
                    }
                    nodes.forEach { n ->
                        val p = pos(n); val s = size[n.ring]!!
                        Column(Modifier.offset((p.x - 40).dp, (p.y - s.value / 2).dp).width(80.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                            BrandIcon(n.icon, size = s)
                            Text(n.label, color = Color(0xFFE3E6FF), fontSize = if (n.ring == 1) 12.sp else 10.5.sp, lineHeight = 12.sp,
                                textAlign = TextAlign.Center, fontWeight = if (n.ring == 1) FontWeight.SemiBold else FontWeight.Normal)
                        }
                    }
                }
                // The single button that replaces the side menu.
                Box(Modifier.offset((cx - 30).dp, (cy - 30).dp).size(60.dp).clip(CircleShape)
                    .background(Brush.radialGradient(listOf(Color(0xFF6E5BFF), Color(0xFF2A2F8F))))
                    .border(1.5.dp, Color(0xFF9DA8FF), CircleShape), contentAlignment = Alignment.Center) {
                    if (open) AppIcon(AppSymbol.Close, Modifier.size(28.dp)) else BrandIcon(Res.drawable.ic_tiles, size = 40.dp)
                }
            }
        }
    }

    /** The open Files tab behind the web: tabs, path and a few rows. */
    @Composable
    private fun AppBehind(modifier: Modifier) {
        val scheme = MaterialTheme.colorScheme
        Column(modifier.padding(top = 36.dp)) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Внутренняя память", color = scheme.onSurface, fontSize = 22.sp)
            }
            Row(Modifier.padding(horizontal = 8.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                Tab("DCIM", true); Tab("Download", false); Tab("Музыка", false)
                Text("+", color = scheme.onSurfaceVariant, fontSize = 22.sp, modifier = Modifier.padding(horizontal = 10.dp))
            }
            Spacer(Modifier.height(12.dp))
            listOf(Res.drawable.ic_folder to "Camera", Res.drawable.ic_folder to "Screenshots", Res.drawable.ic_images to "IMG_2041.jpg",
                Res.drawable.ic_images to "IMG_2042.jpg", Res.drawable.ic_video to "VID_0310.mp4", Res.drawable.ic_images to "IMG_2047.jpg",
                Res.drawable.ic_documents to "Договор.pdf", Res.drawable.ic_music to "voice.ogg", Res.drawable.ic_images to "IMG_2050.jpg",
                Res.drawable.ic_folder to "WhatsApp").forEach { (icon, name) ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 3.dp).clip(RoundedCornerShape(14.dp))
                    .background(scheme.surfaceContainer).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    BrandIcon(icon, size = 40.dp); Spacer(Modifier.width(12.dp))
                    Column { Text(name, color = scheme.onSurface, fontSize = 16.sp); Text("26.11 · 3,2 МБ", color = scheme.onSurfaceVariant, fontSize = 12.sp) }
                }
            }
        }
    }

    @Composable
    private fun Tab(title: String, on: Boolean) {
        val scheme = MaterialTheme.colorScheme
        Column(Modifier.clip(RoundedCornerShape(topStart = 10.dp, topEnd = 10.dp))
            .background(if (on) scheme.surfaceContainerHighest else scheme.surfaceContainerLow).width(100.dp)) {
            Text(title, color = if (on) scheme.onSurface else scheme.onSurfaceVariant, modifier = Modifier.padding(10.dp), fontSize = 14.sp)
            Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) scheme.primary else Color.Transparent))
        }
    }

}
