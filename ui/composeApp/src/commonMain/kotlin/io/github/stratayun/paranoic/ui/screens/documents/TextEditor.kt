package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch

// Bound Compose text layout independently of Core's 16 MiB encoded-file limit.
private const val MAX_EDITOR_CHARS = 65_536

@Composable
fun TextEditor(core: CoreGateway, file: FileItem, parent: Location?, guard: DocumentExitGuard, onClose: () -> Unit) {
    key(core, file.location) {
        var paged by remember { mutableStateOf(!file.sizeKnown || file.bytes > 262_144uL) }
        if (paged) PagedTextViewer(core, file, onClose)
        else TextEditorContent(core, file, parent, guard, onClose, onReadPaged = { paged = true })
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TextEditorContent(core: CoreGateway, file: FileItem, parent: Location?, guard: DocumentExitGuard,
    onClose: () -> Unit, onReadPaged: () -> Unit) {
    val scope = rememberCoroutineScope()
    var document by remember { mutableStateOf<TextDocument?>(null) }
    var text by remember { mutableStateOf("") }
    var baseline by remember { mutableStateOf("") }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var loading by remember { mutableStateOf(true) }
    var attempt by remember { mutableIntStateOf(0) }
    var changed by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf(false) }
    var copyName by remember { mutableStateOf("") }
    var copyDialog by remember { mutableStateOf(false) }
    var copySaved by remember { mutableStateOf<String?>(null) }
    var details by remember { mutableStateOf(false) }
    var textScale by remember { mutableFloatStateOf(1f) }
    val textTransform = rememberTransformableState { factor, _, _ -> textScale = (textScale * factor).coerceIn(1f, 3f) }

    LaunchedEffect(attempt) {
        loading = true
        document = null
        text = ""
        baseline = ""
        fault = null
        changed = false
        saved = false
        copySaved = null
        guard.dirty = false
        guard.uncertain = false
        val result = core.textOpen(file.location)
        currentCoroutineContext().ensureActive()
        when (result) {
            is CoreResult.Value -> {
                if (result.value.text.length <= MAX_EDITOR_CHARS) {
                    document = result.value
                    text = result.value.text
                    baseline = text
                } else onReadPaged()
            }
            is CoreResult.Failure -> if (result.fault == CoreFault.Limit) onReadPaged() else fault = result.fault
        }
        loading = false
    }
    DisposableEffect(Unit) {
        onDispose {
            document = null
            text = ""
            baseline = ""
            guard.dirty = false
            guard.cancel()
        }
    }

    fun save(copy: Boolean) {
        val opened = document ?: return
        if (opened.text.length > MAX_EDITOR_CHARS || guard.busy || guard.uncertain ||
            (copy && parent == null) || (!copy && changed)) return
        val submitted = text
        val name = copyName
        guard.busy = true // before dispatch: a second click cannot submit another write
        fault = null
        saved = false
        copySaved = null
        copyDialog = false
        scope.launch {
            try {
                val result = if (copy) core.textSaveCopy(parent ?: return@launch, name, submitted)
                    else core.textSave(file.location, opened.version, submitted)
                currentCoroutineContext().ensureActive()
                when (result) {
                    is CoreResult.Value -> {
                        if (copy) copySaved = name
                        else {
                            // Adopt only the version returned for these submitted bytes.
                            document = opened.copy(text = submitted, version = result.value)
                            baseline = submitted
                            guard.dirty = text != baseline
                            saved = true
                        }
                    }
                    is CoreResult.Failure -> {
                        fault = result.fault
                        if (result.fault == CoreFault.Changed) changed = true
                        // Never re-submit a write whose outcome may be unknown.
                        guard.uncertain = result.fault in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
                            CoreFault.Disconnected, CoreFault.Cancelled)
                    }
                }
            } catch (cancelled: CancellationException) {
                guard.uncertain = true
                throw cancelled
            } finally { guard.busy = false }
        }
    }

    Surface(Modifier.fillMaxSize()) {
    BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
        val controlsHeight = maxHeight * 0.30f
        Column(Modifier.fillMaxSize().padding(horizontal = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onClose, enabled = !guard.busy) { Text(tr("Назад", "Back")) }
                Text(displayName(file.name), Modifier.weight(1f).sectionHeading(),
                    style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                TextButton(onClick = { details = !details }) { Text(tr("Ещё", "More")) }
            }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (details || document != null || fault != null || changed || guard.uncertain || guard.busy || saved || copySaved != null) {
                Column(Modifier.fillMaxWidth().heightIn(max = controlsHeight).verticalScroll(rememberScrollState())) {
                    if (document != null) Text(tr("При блокировке и уходе в фон несохранённые правки пропадут.",
                        "Unsaved edits are lost on lock or when the app enters the background."), style = MaterialTheme.typography.bodySmall)
                    fault?.let { FaultNotice(it, message = if (it == CoreFault.Limit) tr(
                        "Редактирование доступно до 65 536 символов; большие файлы открываются постранично только для чтения.",
                        "Editing is available up to 65,536 characters; larger files open in a read-only paged viewer.")
                        else it.label()) }
                    if (changed) Text(tr("Файл изменён извне. Сохраните правки копией или перечитайте файл.",
                        "The file changed elsewhere. Save your edits as a copy or reload the file."))
                    if (guard.uncertain) Text(tr("Результат записи неизвестен. Проверьте папку и журнал, затем перечитайте файл. Повтор отключён.",
                        "The write outcome is unknown. Inspect the folder and journal, then reload. Retry is disabled."),
                        color = MaterialTheme.colorScheme.error)
                    if (guard.busy) Text(tr("Сохранение… Дождитесь результата.", "Saving… Wait for the result."))
                    if (saved) Text(tr("Сохранено.", "Saved."))
                    copySaved?.let { Text(tr("Копия сохранена: ", "Copy saved: ") + displayName(it) +
                        tr(". Открыт исходный файл.", ". The original file remains open.")) }
                    if (details) {
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            TextButton(enabled = document != null && parent != null && !guard.busy && !guard.uncertain,
                                onClick = { copyDialog = true }) { Text(tr("Сохранить копию…", "Save a copy…")) }
                            TextButton(enabled = !loading && !guard.busy,
                                onClick = { guard.request { attempt++ } }) { Text(tr("Перечитать", "Reload")) }
                            TextButton(enabled = textScale > 1f, onClick = { textScale = (textScale / 1.25f).coerceAtLeast(1f) }) { Text(tr("− Шрифт", "− Font")) }
                            TextButton(enabled = textScale < 3f, onClick = { textScale = (textScale * 1.25f).coerceAtMost(3f) }) { Text(tr("+ Шрифт", "+ Font")) }
                        }
                        document?.let { opened ->
                            val encoding = when (opened.encoding) {
                                TextEncoding.Utf8 -> "UTF-8"; TextEncoding.Utf8Bom -> "UTF-8 BOM"
                                TextEncoding.Utf16Le -> "UTF-16 LE"; TextEncoding.Utf16Be -> "UTF-16 BE"
                            }
                            Text(encoding + if (guard.dirty) tr(" · изменён", " · modified") else "",
                                style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
            Box(Modifier.weight(1f).fillMaxWidth().transformable(textTransform, canPan = { false })) {
                document?.let { opened ->
                    OutlinedTextField(value = text, onValueChange = { next ->
                        if (next.length <= MAX_EDITOR_CHARS) {
                            text = next
                            guard.dirty = text != baseline
                            saved = false
                            copySaved = null
                            if (fault == CoreFault.Limit) fault = null
                        } else fault = CoreFault.Limit
                    }, readOnly = guard.busy || guard.uncertain, modifier = Modifier.fillMaxSize(),
                        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace,
                            fontSize = MaterialTheme.typography.bodyMedium.fontSize * textScale),
                        label = { Text(tr("Текст документа", "Document text")) })
                }
            }
            // Outside all scroll containers; remains above the IME and system navigation.
            Surface(Modifier.fillMaxWidth(), tonalElevation = 3.dp) {
                Row(Modifier.padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(modifier = Modifier.weight(1f),
                        enabled = document != null && guard.dirty && !guard.busy && !guard.uncertain && !changed,
                        onClick = { save(false) }) { Text(tr("Сохранить", "Save")) }
                    OutlinedButton(modifier = Modifier.weight(1f), enabled = !guard.busy,
                        onClick = onClose) { Text(tr("Не сохранять", "Don't save")) }
                }
            }
        }
    }
    }
    if (copyDialog) AlertDialog(onDismissRequest = { copyDialog = false },
        title = { Text(tr("Новая копия в текущей папке", "New copy in the current folder")) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("UTF-8. Существующий файл не заменяется.", "UTF-8. Existing files are never replaced."))
                OutlinedTextField(value = copyName, onValueChange = { if (it.length <= 255) copyName = it },
                    singleLine = true, label = { Text(tr("Имя нового файла", "New file name")) })
            }
        }, confirmButton = { TextButton(enabled = validCopyName(copyName) && !guard.busy,
            onClick = { save(true) }) { Text(tr("Сохранить копию", "Save copy")) } },
        dismissButton = { TextButton(onClick = { copyDialog = false }) { Text(tr("Отмена", "Cancel")) } })
}

private fun validCopyName(name: String) = name.isNotBlank() && name != "." && name != ".." &&
    name.none { it == '/' || it == '\\' || it.isISOControl() }
