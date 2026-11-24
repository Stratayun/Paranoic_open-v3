package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

// A native row has at most 16 KiB of text. Only this bounded window is retained.
private const val PAGE_ROWS = 128

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun PagedTextViewer(core: CoreGateway, file: FileItem, onClose: () -> Unit) {
    var pager by remember { mutableStateOf<TextPager?>(null) }
    var lines by remember { mutableStateOf<List<String>>(emptyList()) }
    var first by remember { mutableLongStateOf(0) }
    var jump by remember { mutableStateOf("") }
    var attempt by remember { mutableIntStateOf(0) }
    var opening by remember { mutableStateOf(true) }
    var reading by remember { mutableStateOf(false) }
    var readRevision by remember { mutableLongStateOf(0) }
    var invalidated by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var scale by remember { mutableFloatStateOf(1f) }
    val transform = rememberTransformableState { factor, _, _ -> scale = (scale * factor).coerceIn(1f, 3f) }
    val list = rememberLazyListState()

    LaunchedEffect(core, file.location, attempt) {
        pager = null; lines = emptyList(); first = 0; jump = ""
        readRevision++
        opening = true; reading = false; invalidated = false; fault = null
        var owned: TextPager? = null
        try {
            val result = core.textPagerOpen(file.location)
            // Register ownership before the cancellation check; even a custom
            // gateway that delivers after cancellation must have its handle closed.
            if (result is CoreResult.Value) owned = result.value
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    if (result.value.rows < 0 || result.value.handle.isBlank()) {
                        fault = CoreFault.Unavailable
                    } else pager = result.value
                }
                is CoreResult.Failure -> fault = result.fault
            }
            opening = false
            if (pager != null) awaitCancellation()
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable; opening = false }
        finally { owned?.let { core.textPagerClose(it.handle) } }
    }
    val opened = pager
    LaunchedEffect(opened?.handle, first) {
        val request = ++readRevision
        lines = emptyList()
        val current = opened ?: return@LaunchedEffect
        if (invalidated || current.rows == 0L) return@LaunchedEffect
        reading = true; fault = null
        try {
            val count = minOf(PAGE_ROWS.toLong(), current.rows - first).toInt()
            if (first < 0 || count <= 0) { fault = CoreFault.InvalidName; invalidated = true; return@LaunchedEffect }
            val result = core.textPagerRead(current.handle, first, count)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    if (result.value.size != count) { fault = CoreFault.Changed; invalidated = true }
                    else { lines = result.value; list.scrollToItem(0) }
                }
                is CoreResult.Failure -> { fault = result.fault; invalidated = true }
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable; invalidated = true }
        finally { if (request == readRevision) reading = false }
        if (invalidated) core.textPagerClose(current.handle)
    }
    DisposableEffect(Unit) { onDispose { lines = emptyList(); pager = null; jump = "" } }

    Surface(Modifier.fillMaxSize()) {
        BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding()) {
            val controlsMaxHeight = maxHeight * 0.40f
            Column(Modifier.fillMaxSize().padding(horizontal = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    TextButton(onClick = onClose) { Text(tr("Назад", "Back")) }
                    Text(displayName(file.name), Modifier.weight(1f).sectionHeading(), maxLines = 1,
                        overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleSmall)
                }
                if (opening || reading) LinearProgressIndicator(Modifier.fillMaxWidth())
                Column(Modifier.fillMaxWidth().heightIn(max = controlsMaxHeight).verticalScroll(rememberScrollState())) {
                    Text(tr("Только чтение · до 1 ГиБ. Сначала строится индекс; длинные строки делятся на фрагменты. Частичный просмотр нельзя сохранить поверх файла.",
                        "Read only · up to 1 GiB. The index is built first; long lines are split into fragments. A partial view cannot overwrite the file."),
                        style = MaterialTheme.typography.bodySmall)
                    fault?.let { FaultNotice(it, message = when {
                        it == CoreFault.Limit -> tr("Превышен предел чтения: размер или число строк/фрагментов.", "The reading limit was exceeded: file size or number of rows/fragments.")
                        invalidated -> tr("Чтение прервано. Файл мог измениться, либо сеанс закрыт. Перечитайте файл; старые страницы не смешиваются с новыми.",
                            "Reading stopped. The file may have changed or the session closed. Reload the file; old pages are not mixed with new ones.")
                        else -> it.label()
                    }) }
                    opened?.let { info ->
                        Text(if (info.rows == 0L) tr("Текст пуст.", "The text is empty.")
                            else if (lines.isEmpty()) tr("Всего строк/фрагментов: ${info.rows}", "Total rows/fragments: ${info.rows}")
                            else tr("Строки/фрагменты: ${first + 1}–${first + lines.size} из ${info.rows}",
                                "Rows/fragments: ${first + 1}–${first + lines.size} of ${info.rows}"),
                            style = MaterialTheme.typography.bodySmall)
                        if (info.rows > 0) FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            TextButton(enabled = !reading && !invalidated && first > 0,
                                onClick = { first = (first - PAGE_ROWS).coerceAtLeast(0) }) { Text(tr("Предыдущие", "Previous")) }
                            TextButton(enabled = !reading && !invalidated && first < info.rows - PAGE_ROWS,
                                onClick = { first += PAGE_ROWS }) { Text(tr("Следующие", "Next")) }
                            OutlinedTextField(jump, { value -> if (value.length <= 19 && value.all { it in '0'..'9' }) jump = value },
                                Modifier.width(160.dp), singleLine = true, label = { Text(tr("Номер фрагмента", "Fragment number")) },
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
                            val target = jump.toLongOrNull()?.takeIf { it >= 1 && it <= info.rows }
                            TextButton(enabled = !reading && !invalidated && target != null,
                                onClick = { target?.let { first = it - 1; jump = "" } }) { Text(tr("Перейти", "Go")) }
                        }
                    }
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        TextButton(enabled = !opening, onClick = { attempt++ }) { Text(tr("Перечитать", "Reload")) }
                        TextButton(enabled = scale > 1f, onClick = { scale = (scale / 1.25f).coerceAtLeast(1f) }) { Text(tr("− Шрифт", "− Font")) }
                        TextButton(enabled = scale < 3f, onClick = { scale = (scale * 1.25f).coerceAtMost(3f) }) { Text(tr("+ Шрифт", "+ Font")) }
                    }
                }
                LazyColumn(Modifier.weight(1f).fillMaxWidth().transformable(transform, canPan = { false }), state = list) {
                    itemsIndexed(lines, key = { index, _ -> first + index }) { _, line ->
                        SelectionContainer { Text(line.ifEmpty { " " }, Modifier.fillMaxWidth(),
                            fontFamily = FontFamily.Monospace, fontSize = 14.sp * scale) }
                    }
                }
            }
        }
    }
}
