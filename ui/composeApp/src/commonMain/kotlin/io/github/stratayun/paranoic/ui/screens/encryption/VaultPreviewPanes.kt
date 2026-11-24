package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.screens.documents.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

private const val IMAGE_LIMIT = 15 * 1024 * 1024

@Composable
internal fun VaultImagePane(gateway: EncryptedFolderGateway, handle: String, path: List<String>, entry: VaultEntry,
    image: @Composable (ByteArray, Modifier) -> Unit, onClose: () -> Unit) {
    var bytes by remember { mutableStateOf<ByteArray?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    val zoom = remember { DocumentZoom() }
    LaunchedEffect(gateway, handle, path, entry.name) {
        when (val result = readImage(gateway, handle, path, entry.name)) {
            is CoreResult.Value -> {
                try { currentCoroutineContext().ensureActive(); bytes = result.value }
                catch (cancelled: CancellationException) { result.value.fill(0); throw cancelled }
            }
            is CoreResult.Failure -> fault = result.fault
        }
    }
    DisposableEffect(Unit) { onDispose { bytes?.fill(0); bytes = null } }
    DocumentFrame(entry.name, onClose, controls = {
        ZoomControls(zoom)
        Text(tr("В памяти: до 15 МиБ и 4096×4096 пикселей.", "In memory: up to 15 MiB and 4096×4096 pixels."), style = MaterialTheme.typography.bodySmall)
        fault?.let { FaultNotice(it) }
    }) {
        ZoomableDocument(zoom) { bytes?.let { image(it, Modifier.fillMaxSize()) } }
        if (bytes == null && fault == null) LinearProgressIndicator(Modifier.fillMaxWidth())
    }
}

private suspend fun readImage(gateway: EncryptedFolderGateway, handle: String, path: List<String>, name: String): CoreResult<ByteArray> {
    var buffer: ByteArray? = null
    var delivered = false
    try {
        val result = withContext(Dispatchers.IO) {
            when (val opened = gateway.openStream(handle, path, name)) {
                is CoreResult.Failure -> opened
                is CoreResult.Value -> {
                    val source = opened.value
                    try {
                        if (source.length !in 0..IMAGE_LIMIT.toLong()) CoreResult.Failure(CoreFault.Limit)
                        else {
                            val bytes = ByteArray(source.length.toInt()).also { buffer = it }
                            var offset = 0
                            while (offset < bytes.size) {
                                ensureActive()
                                val chunk = source.readAt(offset.toLong(), minOf(65_536, bytes.size - offset))
                                try {
                                    if (chunk.isEmpty() || chunk.size > bytes.size - offset) throw IllegalStateException("invalid protected image length")
                                    chunk.copyInto(bytes, offset); offset += chunk.size
                                } finally { chunk.fill(0) }
                            }
                            CoreResult.Value(bytes)
                        }
                    } finally { withContext(NonCancellable) { source.close() } }
                }
            }
        }
        currentCoroutineContext().ensureActive()
        delivered = result is CoreResult.Value
        return result
    } catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { return CoreResult.Failure(CoreFault.Unavailable) }
    finally { if (!delivered) buffer?.fill(0) }
}

@Composable
internal fun VaultPreviewPane(core: CoreGateway, gateway: EncryptedFolderGateway, handle: String, path: List<String>, entry: VaultEntry,
    image: @Composable (ByteArray, Modifier) -> Unit,
    player: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit, onClose: () -> Unit) {
    val source = remember(core, gateway, handle, path, entry.name) { VaultDocumentGateway(core, gateway, handle, path, entry) }
    FileViewer(source, source.file, onClose, pngImage = image, mediaPlayer = player)
}
