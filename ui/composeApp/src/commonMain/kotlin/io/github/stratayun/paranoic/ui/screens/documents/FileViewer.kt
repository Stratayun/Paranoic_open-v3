package io.github.stratayun.paranoic.ui.screens.documents

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** Views bytes returned by Rust; no file paths, external intents or plaintext cache. */
@Composable
fun FileViewer(core: CoreGateway, file: FileItem, onClose: () -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit,
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit,
    textParent: Location? = null, exitGuard: DocumentExitGuard = remember { DocumentExitGuard() }) {
    key(core, file.location) {
        when (file.name.substringAfterLast('.', "").lowercase()) {
            "txt", "md", "markdown" -> TextEditor(core, file, textParent, exitGuard, onClose)
            // Extension selects a screen only; Core validates the actual package.
            "docx", "xlsx", "ods" -> OfficeViewer(core, file, onClose)
            else -> PreviewFile(core, file, onClose, pngImage, mediaPlayer)
        }
    }
}

@Composable
private fun PreviewFile(core: CoreGateway, file: FileItem, onClose: () -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit,
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit) {
    var preview by remember { mutableStateOf<PreviewInfo?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    LaunchedEffect(Unit) {
        val result = core.inspect(file.location)
        currentCoroutineContext().ensureActive()
        when (result) {
            is CoreResult.Value -> preview = result.value
            is CoreResult.Failure -> fault = result.fault
        }
    }
    val info = preview
    when (info?.kind) {
        PreviewKind.Pdf -> PdfViewer(core, file, onClose, pngImage)
        PreviewKind.Image -> ImageViewer(core, file, onClose, pngImage)
        PreviewKind.Audio, PreviewKind.Video -> {
            val zoom = remember { DocumentZoom() }
            DocumentFrame(file.name, onClose, controls = {
                if (info.kind == PreviewKind.Video) ZoomControls(zoom)
            }) {
                ZoomableDocument(zoom) { mediaPlayer(core, file.location, info, Modifier.fillMaxSize()) }
            }
        }
        else -> DocumentFrame(file.name, onClose) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                when {
                    fault != null -> FaultNotice(fault!!)
                    info == null -> CircularProgressIndicator()
                    else -> FaultNotice(CoreFault.Unsupported)
                }
            }
        }
    }
}

@Composable
private fun ImageViewer(core: CoreGateway, file: FileItem, onClose: () -> Unit,
    image: @Composable (ByteArray, Modifier) -> Unit) {
    var png by remember { mutableStateOf<ByteArray?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    val zoom = remember { DocumentZoom() }
    LaunchedEffect(Unit) {
        when (val result = core.imagePng(file.location, 2048)) {
            is CoreResult.Value -> {
                try { currentCoroutineContext().ensureActive(); png = result.value }
                catch (cancelled: CancellationException) { result.value.fill(0); throw cancelled }
            }
            is CoreResult.Failure -> fault = result.fault
        }
    }
    DisposableEffect(Unit) { onDispose { png?.fill(0); png = null } }
    DocumentFrame(file.name, onClose, controls = { ZoomControls(zoom) }) {
        ZoomableDocument(zoom) {
            val bytes = png
            when {
                bytes != null -> image(bytes, Modifier.fillMaxSize())
                fault != null -> FaultNotice(fault!!, Modifier.align(Alignment.Center))
                else -> CircularProgressIndicator(Modifier.align(Alignment.Center))
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PdfViewer(core: CoreGateway, file: FileItem, onClose: () -> Unit,
    image: @Composable (ByteArray, Modifier) -> Unit) {
    var index by remember { mutableIntStateOf(0) }
    var width by remember { mutableIntStateOf(2048) }
    var page by remember { mutableStateOf<PdfPage?>(null) }
    var pageCount by remember { mutableStateOf<Int?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var loading by remember { mutableStateOf(false) }
    var searchVisible by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var searchRequest by remember { mutableStateOf<Pair<String, Int>?>(null) }
    var matches by remember { mutableStateOf<List<Int>>(emptyList()) }
    var next by remember { mutableStateOf<Int?>(null) }
    var searching by remember { mutableStateOf(false) }
    val zoom = remember { DocumentZoom() }
    DisposableEffect(Unit) { onDispose { page?.png?.fill(0); page = null; core.releasePdf() } }
    LaunchedEffect(index, width) {
        loading = true
        fault = null
        page?.png?.fill(0); page = null
        try {
            when (val result = core.pdfPage(file.location, index, width)) {
                is CoreResult.Value -> {
                    try {
                        currentCoroutineContext().ensureActive()
                        page = result.value
                        pageCount = result.value.pages
                    } catch (cancelled: CancellationException) { result.value.png.fill(0); throw cancelled }
                }
                is CoreResult.Failure -> fault = result.fault
            }
        } finally { loading = false }
    }
    LaunchedEffect(index) { zoom.reset() }
    LaunchedEffect(searchRequest) {
        val request = searchRequest ?: return@LaunchedEffect
        searching = true
        try {
            val result = core.pdfSearch(file.location, request.first, request.second)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    matches = if (request.second == 0) result.value.pages else (matches + result.value.pages).distinct()
                    next = result.value.nextPage
                }
                is CoreResult.Failure -> { fault = result.fault; next = null }
            }
        } finally { searching = false }
    }
    DocumentFrame(file.name, onClose, controls = {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            TextButton(enabled = index > 0 && !loading, onClick = { index-- }) { Text(tr("Предыдущая", "Previous")) }
            Text(tr("Страница ${index + 1} / ${pageCount ?: "…"}", "Page ${index + 1} / ${pageCount ?: "…"}"), Modifier.padding(vertical = 12.dp))
            TextButton(enabled = pageCount != null && index + 1 < (pageCount ?: 0) && !loading, onClick = { index++ }) { Text(tr("Следующая", "Next")) }
            TextButton(onClick = { searchVisible = !searchVisible }) { Text(tr("Поиск", "Search")) }
        }
        ZoomControls(zoom)
        if (searchVisible) {
            OutlinedTextField(value = query, onValueChange = {
                if (it.length <= 256) { query = it; searchRequest = null; matches = emptyList(); next = null }
            }, singleLine = true, label = { Text(tr("Поиск по тексту PDF", "Search PDF text")) }, modifier = Modifier.fillMaxWidth())
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(enabled = query.isNotBlank() && !searching,
                    onClick = { searchRequest = query to 0 }) { Text(tr("Искать", "Find")) }
                next?.let { start -> TextButton(enabled = !searching, onClick = { searchRequest = query to start }) {
                    Text(tr("Искать дальше", "Continue search"))
                } }
                matches.take(32).forEach { found -> TextButton(enabled = !loading, onClick = { index = found }) { Text((found + 1).toString()) } }
                TextButton(enabled = !loading, onClick = { width = if (width == 4096) 2048 else 4096 }) {
                    Text(tr("Чёткость: $width", "Resolution: $width"))
                }
            }
            if (searchRequest != null && !searching) Text(tr(
                "Страниц с совпадением: ${matches.size}" + if (next != null) "; поиск не завершён" else "; поиск завершён",
                "Matching pages: ${matches.size}" + if (next != null) "; search incomplete" else "; search complete"))
        }
        fault?.let { FaultNotice(it) }
    }) {
        ZoomableDocument(zoom) {
            page?.let { rendered ->
                val pageLabel = tr("Страница PDF ${rendered.index + 1}", "PDF page ${rendered.index + 1}")
                image(rendered.png, Modifier.fillMaxSize().clearAndSetSemantics { contentDescription = pageLabel })
            }
        }
        if (loading || searching) LinearProgressIndicator(Modifier.align(Alignment.BottomCenter).fillMaxWidth())
    }
}
