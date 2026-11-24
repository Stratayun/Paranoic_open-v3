package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
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

/** The document fills the viewport. A solid overlay keeps buttons readable over images.
 * System bars/cutouts are consumed here, including Android 15 edge-to-edge windows. */
@Composable
internal fun DocumentFrame(name: String, onClose: () -> Unit,
    controls: @Composable ColumnScope.() -> Unit = {}, content: @Composable BoxScope.() -> Unit) {
    val chrome = remember { ViewerChrome() }
    CompositionLocalProvider(LocalViewerChrome provides chrome) {
        Surface(Modifier.fillMaxSize()) {
            BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding()) {
                val toolbarHeight = maxHeight * 0.50f
                Box(Modifier.fillMaxSize(), content = content)
                if (chrome.visible) Surface(Modifier.align(Alignment.TopCenter).fillMaxWidth(),
                    tonalElevation = 3.dp, shadowElevation = 2.dp) {
                    Column(Modifier.heightIn(max = toolbarHeight).verticalScroll(rememberScrollState()).padding(horizontal = 8.dp)) {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            TextButton(onClick = onClose) { Text(tr("Назад", "Back")) }
                            Text(displayName(name), Modifier.weight(1f).sectionHeading(),
                                style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            TextButton(onClick = chrome::toggle) { Text(tr("Скрыть", "Hide")) }
                        }
                        controls()
                    }
                } else {
                    // A keyboard/TalkBack route remains available when gestures hide the overlay.
                    Surface(Modifier.align(Alignment.TopEnd), tonalElevation = 3.dp) {
                        TextButton(onClick = chrome::toggle) { Text(tr("Панель", "Controls")) }
                    }
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
        TextButton(enabled = zoom.scale > 1f, onClick = { zoom.update(1f / 1.5f) }) { Text(tr("− Масштаб", "− Zoom")) }
        TextButton(onClick = zoom::reset) { Text("${(zoom.scale * 100).toInt()}%") }
        TextButton(enabled = zoom.scale < 6f, onClick = { zoom.update(1.5f) }) { Text(tr("+ Масштаб", "+ Zoom")) }
    }
}

@Composable
internal fun ZoomableDocument(zoom: DocumentZoom, modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit) {
    val chrome = LocalViewerChrome.current
    val transform = rememberTransformableState { factor, pan, _ -> zoom.update(factor, pan) }
    Box(modifier.fillMaxSize().clipToBounds().onSizeChanged(zoom::resize)
        .transformable(transform, canPan = { zoom.scale > 1f })
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
