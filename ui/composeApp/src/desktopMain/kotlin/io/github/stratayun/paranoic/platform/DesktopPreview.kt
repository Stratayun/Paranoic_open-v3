package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.awt.SwingPanel
import androidx.compose.ui.graphics.asComposeImageBitmap
import androidx.compose.ui.layout.ContentScale
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.label
import javafx.application.Platform
import javafx.embed.swing.JFXPanel
import javafx.scene.Group
import javafx.scene.Scene
import javafx.scene.media.Media
import javafx.scene.media.MediaPlayer
import javafx.scene.media.MediaView
import javafx.scene.paint.Color
import kotlinx.coroutines.*
import kotlinx.coroutines.swing.Swing
import org.jetbrains.skia.Bitmap as SkiaBitmap
import org.jetbrains.skia.Codec as SkiaCodec
import org.jetbrains.skia.Data as SkiaData
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

@Composable
fun DesktopPng(bytes: ByteArray, modifier: Modifier) {
    DesktopMemoryImage(bytes, modifier, pngOnly = true)
}
@Composable
fun DesktopNoteImage(bytes: ByteArray, modifier: Modifier) {
    DesktopMemoryImage(bytes, modifier, pngOnly = false)
}
@Composable
private fun DesktopMemoryImage(bytes: ByteArray, modifier: Modifier, pngOnly: Boolean) {
    val owned = remember(bytes) { AtomicReference<SkiaBitmap?>() }
    var bitmap by remember(bytes) { mutableStateOf<SkiaBitmap?>(null) }
    var failed by remember(bytes) { mutableStateOf(false) }
    DisposableEffect(owned) { onDispose { owned.getAndSet(null)?.close() } }
    LaunchedEffect(bytes) {
        var pending: SkiaBitmap? = null
        try {
            withContext(Dispatchers.Default) {
                PngDecodeBudget.decode {
                    ensureActive()
                    val size = if (pngOnly) PngBounds.read(bytes) else null
                    SkiaData.makeFromBytes(bytes).use { data ->
                        SkiaCodec.makeFromData(data).use { codec ->
                            val width = codec.imageInfo.width
                            val height = codec.imageInfo.height
                            require(width in 1..4096 && height in 1..4096 &&
                                width.toLong() * height * 4 <= PngBounds.MAX_PIXEL_BYTES) { "Image size limit" }
                            if (size != null) require(width == size.width && height == size.height) { "PNG size mismatch" }
                            val decoded = SkiaBitmap()
                            pending = decoded
                            check(decoded.allocN32Pixels(width, height)) { "Image allocation failed" }
                            // Eagerly decode here: no lazy decode on the render thread.
                            codec.readPixels(decoded)
                            decoded.setImmutable()
                        }
                    }
                }
            }
            ensureActive()
            val result = pending
            owned.set(result)
            pending = null
            bitmap = result
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { failed = true }
        catch (_: OutOfMemoryError) { failed = true }
        finally { pending?.close() }
    }
    val decoded = bitmap
    when {
        decoded != null -> Image(remember(decoded) { decoded.asComposeImageBitmap() }, contentDescription = tr("Изображение", "Image"),
            modifier = modifier.aspectRatio(decoded.width.toFloat() / decoded.height), contentScale = ContentScale.Fit)
        failed -> Text(tr("Не удалось показать изображение: повреждённый файл или превышен лимит.",
            "Image unavailable: invalid data or resource limit exceeded."), modifier = modifier,
            color = MaterialTheme.colorScheme.error)
        else -> Text(tr("Загрузка изображения…", "Loading image…"), modifier = modifier)
    }
}
private class FxSession(val endpoint: LoopbackMedia, val panel: JFXPanel) {
    val player = AtomicReference<MediaPlayer?>()
    val closed = AtomicBoolean(false)
    fun close() {
        if (closed.compareAndSet(false, true)) {
            try { Platform.runLater { player.getAndSet(null)?.dispose(); panel.scene = null } }
            finally { endpoint.close() }
        }
    }
}
@Composable
fun DesktopMediaPlayer(core: CoreGateway, location: Location, info: PreviewInfo, modifier: Modifier) {
    var session by remember(location) { mutableStateOf<FxSession?>(null) }
    var fault by remember(location) { mutableStateOf<CoreFault?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(core, location) {
        var source: MediaSource? = null
        var opened: FxSession? = null
        var endpoint: LoopbackMedia? = null
        try {
            when (val result = core.openMedia(location)) {
                is CoreResult.Failure -> fault = result.fault
                is CoreResult.Value -> {
                    source = result.value
                    val server = withContext(Dispatchers.IO) {
                        LoopbackMedia(result.value, info.mime).also { endpoint = it }
                    }
                    val panel = withContext(Dispatchers.Swing) { JFXPanel() }
                    val active = FxSession(server, panel)
                    opened = active
                    session = active
                    Platform.runLater {
                        if (!active.closed.get()) try {
                            Platform.setImplicitExit(false)
                            val player = MediaPlayer(Media(server.uri.toString()))
                            active.player.set(player)
                            player.setOnError { scope.launch { fault = CoreFault.Unsupported } }
                            val view = MediaView(player).apply { isPreserveRatio = true }
                            val scene = Scene(Group(view), Color.BLACK)
                            view.fitWidthProperty().bind(scene.widthProperty())
                            view.fitHeightProperty().bind(scene.heightProperty())
                            panel.scene = scene
                            // Playback begins only after an explicit Play command.
                        } catch (_: Exception) { scope.launch { fault = CoreFault.Unsupported } }
                    }
                }
            }
            // Hold resources until this particular location leaves the UI.
            awaitCancellation()
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        catch (_: LinkageError) { fault = CoreFault.Unsupported }
        finally {
            withContext(NonCancellable + Dispatchers.IO) {
                if (opened != null) opened?.close() else if (endpoint != null) endpoint?.close() else source?.close()
            }
            session = null
        }
    }
    Column(modifier) {
        fault?.let { Text(it.label(), color = MaterialTheme.colorScheme.error) }
        Row {
            TextButton(enabled = session != null, onClick = { session?.let { s -> Platform.runLater { s.player.get()?.play() } } }) {
                Text(tr("Играть", "Play"))
            }
            TextButton(enabled = session != null, onClick = { session?.let { s -> Platform.runLater { s.player.get()?.pause() } } }) {
                Text(tr("Пауза", "Pause"))
            }
            TextButton(enabled = session != null, onClick = { session?.let { s -> Platform.runLater { s.player.get()?.seek(javafx.util.Duration.ZERO) } } }) {
                Text(tr("В начало", "Restart"))
            }
        }
        session?.let { active -> SwingPanel(factory = { active.panel }, modifier = Modifier.weight(1f).fillMaxWidth()) }
    }
}
