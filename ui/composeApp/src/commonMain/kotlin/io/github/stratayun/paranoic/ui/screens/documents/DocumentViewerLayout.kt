package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.components.AppIcon
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.state.actionDescription
import io.github.stratayun.paranoic.ui.state.displayName
import io.github.stratayun.paranoic.ui.state.sectionHeading
import io.github.stratayun.paranoic.ui.state.tr

/** Shared with the native media controls: their tap/timeout also hides the header. */
@Stable
internal class ViewerChrome {
    var visible by mutableStateOf(true)
    fun toggle() { visible = !visible }
}
internal val LocalViewerChrome = staticCompositionLocalOf<ViewerChrome?> { null }

/** Top bar (back, title, full-screen) and an optional row of controls; the document fills the rest.
 * Nothing overlaps the page. Tap the page or the full-screen icon to hide the bars.
 * System bars/cutouts are consumed here, including Android 15 edge-to-edge windows. */
@Composable
internal fun DocumentFrame(name: String, onClose: () -> Unit,
    controls: @Composable ColumnScope.() -> Unit = {}, content: @Composable BoxScope.() -> Unit) {
    val chrome = remember { ViewerChrome() }
    CompositionLocalProvider(LocalViewerChrome provides chrome) {
        Surface(Modifier.fillMaxSize()) {
            BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding()) {
                val toolbarHeight = maxHeight * 0.40f
                Column(Modifier.fillMaxSize()) {
                    if (chrome.visible) Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surfaceContainer) {
                        Column(Modifier.heightIn(max = toolbarHeight).verticalScroll(rememberScrollState()).padding(horizontal = 4.dp, vertical = 2.dp)) {
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                IconButton(onClick = onClose, modifier = Modifier.actionDescription(tr("Назад", "Back"))) { AppIcon(AppSymbol.Back) }
                                Text(displayName(name), Modifier.weight(1f).sectionHeading(),
                                    style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                IconButton(onClick = chrome::toggle, modifier = Modifier.actionDescription(tr("Во весь экран", "Full screen"))) { AppIcon(AppSymbol.Fullscreen) }
                            }
                            controls()
                        }
                    }
                    Box(Modifier.weight(1f).fillMaxWidth(), content = content)
                }
                if (!chrome.visible) {
                    // A keyboard/TalkBack route remains available when gestures hide the bars.
                    SmallFloatingActionButton(onClick = chrome::toggle, modifier = Modifier.align(Alignment.TopEnd).padding(8.dp)
                        .actionDescription(tr("Показать панель", "Show controls"))) { AppIcon(AppSymbol.Menu) }
                }
            }
        }
    }
}

@Stable
internal class DocumentZoom {
    var scale by mutableFloatStateOf(1f)
        private set
    var offset by mutableStateOf(Offset.Zero)
        private set
    private var viewport = IntSize.Zero
    fun resize(size: IntSize) { viewport = size; update(1f, Offset.Zero) }
    fun update(factor: Float, pan: Offset = Offset.Zero) {
        scale = (scale * factor).coerceIn(1f, 6f)
        val limitX = viewport.width * (scale - 1f) / 2f
        val limitY = viewport.height * (scale - 1f) / 2f
        offset = Offset((offset.x + pan.x).coerceIn(-limitX, limitX),
            (offset.y + pan.y).coerceIn(-limitY, limitY))
    }
    fun reset() { scale = 1f; offset = Offset.Zero }
}

@Composable
internal fun ZoomControls(zoom: DocumentZoom) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(enabled = zoom.scale > 1f, onClick = { zoom.update(1f / 1.5f) },
            modifier = Modifier.actionDescription(tr("Уменьшить", "Zoom out"))) { AppIcon(AppSymbol.ZoomOut) }
        TextButton(onClick = zoom::reset) { Text("${(zoom.scale * 100).toInt()}%") }
        IconButton(enabled = zoom.scale < 6f, onClick = { zoom.update(1.5f) },
            modifier = Modifier.actionDescription(tr("Увеличить", "Zoom in"))) { AppIcon(AppSymbol.ZoomIn) }
    }
}

/** Pinch zooms; at 100 % single-finger drags go to the content, so lists and pages scroll. */
@Composable
internal fun ZoomableDocument(zoom: DocumentZoom, modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit) {
    val chrome = LocalViewerChrome.current
    Box(modifier.fillMaxSize().clipToBounds().onSizeChanged(zoom::resize)
        .pointerInput(zoom) {
            awaitEachGesture {
                awaitFirstDown(requireUnconsumed = false)
                do {
                    val event = awaitPointerEvent()
                    val fingers = event.changes.count { it.pressed }
                    if (fingers >= 2 || zoom.scale > 1f) {
                        val factor = event.calculateZoom()
                        val pan = event.calculatePan()
                        if (factor != 1f || pan != Offset.Zero) {
                            zoom.update(factor, pan)
                            event.changes.forEach { it.consume() }
                        }
                    }
                } while (event.changes.any { it.pressed })
            }
        }
        .pointerInput(chrome, zoom) {
            detectTapGestures(onTap = { chrome?.toggle() },
                onDoubleTap = { if (zoom.scale > 1f) zoom.reset() else zoom.update(2f) })
        }) {
        Box(Modifier.fillMaxSize().graphicsLayer {
            scaleX = zoom.scale; scaleY = zoom.scale
            translationX = zoom.offset.x; translationY = zoom.offset.y
        }, content = content)
    }
}

/** Width/height from a PNG IHDR chunk; null when the bytes are not a PNG. */
internal fun pngAspect(png: ByteArray): Float? {
    if (png.size < 24 || png[1] != 'P'.code.toByte() || png[2] != 'N'.code.toByte()) return null
    fun int(at: Int) = (png[at].toInt() and 0xFF shl 24) or (png[at + 1].toInt() and 0xFF shl 16) or
        (png[at + 2].toInt() and 0xFF shl 8) or (png[at + 3].toInt() and 0xFF)
    val w = int(16); val h = int(20)
    return if (w > 0 && h > 0) w.toFloat() / h else null
}
