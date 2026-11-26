package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.components.AppIcon
import androidx.compose.ui.graphics.Color
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
    val textTransform = rememberTransformableState { factor, _, _ -> textScale = (textScale * factor).coerceIn(0.6f, 3f) }

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
            var menu by remember { mutableStateOf(false) }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onClose, enabled = !guard.busy, modifier = Modifier.actionDescription(
                    if (guard.dirty) tr("Назад без сохранения", "Back without saving") else tr("Назад", "Back"))) { AppIcon(AppSymbol.Back) }
                Column(Modifier.weight(1f)) {
                    Text(displayName(file.name), Modifier.sectionHeading(),
                        style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(when {
                        guard.busy -> tr("Сохранение…", "Saving…")
                        guard.dirty -> tr("Изменён · не сохранён", "Modified · not saved")
                        saved -> tr("Сохранено", "Saved")
                        else -> document?.let { when (it.encoding) {
                            TextEncoding.Utf8 -> "UTF-8"; TextEncoding.Utf8Bom -> "UTF-8 BOM"
                            TextEncoding.Utf16Le -> "UTF-16 LE"; TextEncoding.Utf16Be -> "UTF-16 BE" } }.orEmpty()
                    }, style = MaterialTheme.typography.labelMedium,
                        color = if (guard.dirty) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.onSurfaceVariant)
                }
                FilledTonalIconButton(enabled = document != null && guard.dirty && !guard.busy && !guard.uncertain && !changed,
                    onClick = { save(false) }, modifier = Modifier.actionDescription(tr("Сохранить", "Save"))) { AppIcon(AppSymbol.Save) }
                Box {
                    IconButton(onClick = { menu = true }, modifier = Modifier.actionDescription(tr("Ещё", "More"))) { AppIcon(AppSymbol.More) }
                    DropdownMenu(menu, { menu = false }) {
                        DropdownMenuItem(text = { Text(tr("Сохранить копию…", "Save a copy…")) }, leadingIcon = { AppIcon(AppSymbol.Copy) },
                            enabled = document != null && parent != null && !guard.busy && !guard.uncertain, onClick = { menu = false; copyDialog = true })
                        DropdownMenuItem(text = { Text(tr("Перечитать с диска", "Reload from disk")) }, leadingIcon = { AppIcon(AppSymbol.Refresh) },
                            enabled = !loading && !guard.busy, onClick = { menu = false; guard.request { attempt++ } })
                        DropdownMenuItem(text = { Text(tr("Крупнее шрифт", "Larger font")) }, leadingIcon = { AppIcon(AppSymbol.ZoomIn) },
                            enabled = textScale < 3f, onClick = { textScale = (textScale * 1.25f).coerceAtMost(3f) })
                        DropdownMenuItem(text = { Text(tr("Мельче шрифт", "Smaller font")) }, leadingIcon = { AppIcon(AppSymbol.ZoomOut) },
                            enabled = textScale > 0.6f, onClick = { textScale = (textScale / 1.25f).coerceAtLeast(0.6f) })
                    }
                }
            }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (fault != null || changed || guard.uncertain || copySaved != null) {
                Column(Modifier.fillMaxWidth().heightIn(max = controlsHeight).verticalScroll(rememberScrollState())) {
                    fault?.let { FaultNotice(it, message = if (it == CoreFault.Limit) tr(
                        "Редактирование доступно до 65 536 символов; большие файлы открываются постранично только для чтения.",
                        "Editing is available up to 65,536 characters; larger files open in a read-only paged viewer.")
                        else it.label()) }
                    if (changed) Text(tr("Файл изменён извне. Сохраните правки копией или перечитайте файл.",
                        "The file changed elsewhere. Save your edits as a copy or reload the file."))
                    if (guard.uncertain) Text(tr("Результат записи неизвестен. Проверьте папку и журнал, затем перечитайте файл. Повтор отключён.",
                        "The write outcome is unknown. Inspect the folder and journal, then reload. Retry is disabled."),
                        color = MaterialTheme.colorScheme.error)
                    copySaved?.let { Text(tr("Копия сохранена: ", "Copy saved: ") + displayName(it) +
                        tr(". Открыт исходный файл.", ". The original file remains open.")) }
                }
            }
            Box(Modifier.weight(1f).fillMaxWidth().transformable(textTransform, canPan = { false })) {
                document?.let { opened ->
                    TextField(value = text, onValueChange = { next ->
                        if (next.length <= MAX_EDITOR_CHARS) {
                            text = next
                            guard.dirty = text != baseline
                            saved = false
                            copySaved = null
                            if (fault == CoreFault.Limit) fault = null
                        } else fault = CoreFault.Limit
                    }, readOnly = guard.busy || guard.uncertain, modifier = Modifier.fillMaxSize(),
                        textStyle = MaterialTheme.typography.bodyLarge.copy(
                            fontSize = MaterialTheme.typography.bodyLarge.fontSize * textScale),
                        placeholder = { Text(tr("Начните писать…", "Start typing…")) },
                        shape = MaterialTheme.shapes.large,
                        colors = TextFieldDefaults.colors(focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                            disabledIndicatorColor = Color.Transparent,
                            focusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
                            unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow))
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
