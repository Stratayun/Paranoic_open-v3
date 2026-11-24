package io.github.stratayun.paranoic.platform

import android.graphics.BitmapFactory
import android.graphics.Bitmap
import android.net.Uri
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.PlaybackException
import androidx.media3.datasource.BaseDataSource
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.ProgressiveMediaSource
import androidx.media3.ui.PlayerView
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import io.github.stratayun.paranoic.ui.state.UiLanguage
import io.github.stratayun.paranoic.ui.screens.documents.LocalViewerChrome
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.IOException
import java.util.concurrent.atomic.AtomicReference

@Composable
fun AndroidPng(bytes: ByteArray, modifier: Modifier) {
    AndroidMemoryImage(bytes, modifier, pngOnly = true)
}
@Composable
fun AndroidNoteImage(bytes: ByteArray, modifier: Modifier) {
    AndroidMemoryImage(bytes, modifier, pngOnly = false)
}
@Composable
private fun AndroidMemoryImage(bytes: ByteArray, modifier: Modifier, pngOnly: Boolean) {
    val owned = remember(bytes) { AtomicReference<Bitmap?>() }
    var bitmap by remember(bytes) { mutableStateOf<Bitmap?>(null) }
    var failed by remember(bytes) { mutableStateOf(false) }
    // Own the published result even if the UI leaves before recomposing it.
    DisposableEffect(owned) { onDispose { owned.getAndSet(null)?.recycle() } }
    LaunchedEffect(bytes) {
        // Assigned inside withContext: a cancelled dispatcher handoff must not
        // drop the only reference to an already allocated native bitmap.
        var pending: Bitmap? = null
        try {
            withContext(Dispatchers.Default) {
                PngDecodeBudget.decode {
                    ensureActive()
                    val size = if (pngOnly) PngBounds.read(bytes) else null
                    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
                    require(bounds.outWidth in 1..4096 && bounds.outHeight in 1..4096 &&
                        bounds.outWidth.toLong() * bounds.outHeight * 4 <= PngBounds.MAX_PIXEL_BYTES) { "Image size limit" }
                    if (size != null) require(bounds.outWidth == size.width && bounds.outHeight == size.height) { "PNG size mismatch" }
                    val options = BitmapFactory.Options().apply {
                        inPreferredConfig = Bitmap.Config.ARGB_8888
                        inScaled = false
                    }
                    pending = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
                        ?: throw IllegalArgumentException("PNG decode failed")
                    val decoded = checkNotNull(pending)
                    require(decoded.width == bounds.outWidth && decoded.height == bounds.outHeight
                        && decoded.allocationByteCount.toLong() <= PngBounds.MAX_PIXEL_BYTES) { "PNG allocation limit" }
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
        finally { pending?.recycle() }
    }
    val decoded = bitmap
    when {
        decoded != null -> Image(remember(decoded) { decoded.asImageBitmap() }, contentDescription = tr("Изображение", "Image"),
            modifier = modifier.aspectRatio(decoded.width.toFloat() / decoded.height), contentScale = ContentScale.Fit)
        failed -> Text(tr("Не удалось показать изображение: повреждённый файл или превышен лимит.",
            "Image unavailable: invalid data or resource limit exceeded."), modifier = modifier)
        else -> Text(tr("Загрузка изображения…", "Loading image…"), modifier = modifier)
    }
}

@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
private class CoreDataSource(private val core: CoreGateway, private val location: Location) : BaseDataSource(false) {
    private var source: MediaSource? = null
    private var position = 0L
    private var remaining = 0L
    private var opened = false
    override fun getUri(): Uri = Uri.parse("paranoic://media")
    override fun open(dataSpec: DataSpec): Long {
        transferInitializing(dataSpec)
        val reader = runBlocking {
            when (val result = core.openMedia(location)) {
                is CoreResult.Value -> result.value
                is CoreResult.Failure -> throw IOException("media grant unavailable")
            }
        }
        if (dataSpec.position < 0 || dataSpec.position > reader.length) { reader.close(); throw IOException("invalid range") }
        source = reader
        position = dataSpec.position
        remaining = reader.length - position
        if (dataSpec.length != C.LENGTH_UNSET.toLong()) remaining = minOf(remaining, dataSpec.length)
        opened = true
        transferStarted(dataSpec)
        return remaining
    }
    override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
        if (length == 0) return 0
        if (remaining == 0L) return C.RESULT_END_OF_INPUT
        val reader = source ?: throw IOException("closed media")
        val count = minOf(length.toLong(), remaining, 1048576L).toInt()
        val data = reader.readAt(position, count)
        if (data.isEmpty()) throw IOException("media truncated")
        data.copyInto(buffer, offset)
        position += data.size; remaining -= data.size
        bytesTransferred(data.size)
        val read = data.size; data.fill(0)
        return read
    }
    override fun close() {
        source?.close(); source = null
        if (opened) { opened = false; transferEnded() }
    }
}
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
@Composable
fun AndroidMediaPlayer(core: CoreGateway, location: Location, info: PreviewInfo, modifier: Modifier) {
    val context = LocalContext.current
    val chrome = LocalViewerChrome.current
    val language = LocalUiLanguage.current
    val playerContext = remember(context, language) {
        val config = android.content.res.Configuration(context.resources.configuration)
        config.setLocale(java.util.Locale.forLanguageTag(if (language == UiLanguage.Russian) "ru" else "en"))
        context.createConfigurationContext(config)
    }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var failed by remember(location) { mutableStateOf(false) }
    // A released ExoPlayer cannot be reused: create one per STARTED period,
    // release it on STOP and resume from the saved position afterwards.
    var player by remember(core, location) { mutableStateOf<ExoPlayer?>(null) }
    var position by remember(core, location) { mutableStateOf(0L) }
    fun create(): ExoPlayer = ExoPlayer.Builder(context).build().apply {
        addListener(object : Player.Listener {
            override fun onPlayerError(error: PlaybackException) { failed = true }
        })
        val factory = DataSource.Factory { CoreDataSource(core, location) }
        setMediaSource(ProgressiveMediaSource.Factory(factory).createMediaSource(
            MediaItem.Builder().setUri("paranoic://media").setMimeType(info.mime).build()))
        seekTo(position)
        prepare()
    }
    DisposableEffect(core, location, lifecycle) {
        fun release() { player?.let { position = it.currentPosition; it.release() }; player = null }
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) player = create()
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> if (player == null) { failed = false; player = create() }
                Lifecycle.Event.ON_STOP -> release()
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer); release() }
    }
    Column(modifier) {
        if (failed) Text(tr("Не удалось воспроизвести файл: кодек, доступ или поток недоступен.",
            "Playback failed: codec, access or stream unavailable."))
        key(language) {
            AndroidView(factory = { PlayerView(playerContext).apply {
                this.player = player; useController = true; isFocusable = true
                setControllerVisibilityListener(PlayerView.ControllerVisibilityListener { visibility ->
                    chrome?.visible = visibility == android.view.View.VISIBLE
                })
            } }, modifier = Modifier.fillMaxSize(), onReset = null, onRelease = { it.player = null },
                update = {
                    it.player = player
                    if (chrome?.visible == true) it.showController() else if (chrome != null) it.hideController()
                })
        }
    }
}
