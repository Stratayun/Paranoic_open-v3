package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.components.SecretInput
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

@Composable
internal fun VaultTextPane(session: AppSession, gateway: EncryptedFolderGateway, handle: String, path: List<String>,
    entry: VaultEntry, writable: Boolean, guard: DocumentExitGuard, onBusy: (Boolean) -> Unit, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var document by remember { mutableStateOf<VaultTextDocument?>(null) }
    var text by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var copySaved by remember { mutableStateOf<String?>(null) }
    var saved by remember { mutableStateOf(false) }
    var changed by remember { mutableStateOf(false) }
    var reloadRequired by remember { mutableStateOf(false) }
    fun busy(value: Boolean) { guard.busy = value; onBusy(value) }
    LaunchedEffect(gateway, handle, path, entry.name, attempt) {
        busy(true); document = null; text = ""; fault = null; copySaved = null; guard.dirty = false
        saved = false; changed = false; reloadRequired = false
        try {
            val result = gateway.textOpen(handle, path, entry.name)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> { document = result.value; if (result.value.text.length <= 65_536) text = result.value.text }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { busy(false) }
    }
    DisposableEffect(Unit) { onDispose { document = null; text = ""; name = ""; guard.dirty = false } }
    val opened = document
    val large = (opened?.text?.length ?: 0) > 65_536
    Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding().padding(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row {
            TextButton(enabled = !guard.busy, onClick = onClose) { Text(tr("Назад", "Back")) }
            Text(displayName(entry.name), Modifier.weight(1f).sectionHeading(), maxLines = 1)
            TextButton(enabled = !guard.busy, onClick = { guard.request { attempt++ } }) { Text(tr("Перечитать", "Reload")) }
        }
        if (guard.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        fault?.let { FaultNotice(it) }
        copySaved?.let { Text(tr("Создана зашифрованная копия: ", "Encrypted copy created: ") + displayName(it)) }
        if (saved) Text(tr("Изменения сохранены в исходном файле.", "Changes saved in the original file."))
        if (large) {
            Text(tr("Большой текст — только чтение, до 16 МиБ. Текст показан частями.", "Large text is read only, up to 16 MiB. Text is displayed in sections."))
            val content = opened?.text.orEmpty()
            LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                items((content.length + 2047) / 2048) { index ->
                    val part = remember(content, index) {
                        var start = index * 2048
                        var end = minOf(start + 2048, content.length)
                        if (start > 0 && content[start].isLowSurrogate() && content[start - 1].isHighSurrogate()) start--
                        if (end < content.length && content[end].isLowSurrogate() && content[end - 1].isHighSurrogate()) end--
                        content.substring(start, end)
                    }
                    Text(part, Modifier.fillMaxWidth(), fontFamily = FontFamily.Monospace)
                }
            }
        } else {
            Text(tr("Сохраняется поверх открытой версии в той же кодировке. При блокировке черновик пропадёт.",
                "Saves over the opened version in the same encoding. Locking discards the draft."), style = MaterialTheme.typography.bodySmall)
            if (changed) Text(tr("Файл изменился, пока был открыт. Сохраните черновик копией или перечитайте файл.",
                "The file changed while it was open. Save the draft as a copy or reload the file."), color = MaterialTheme.colorScheme.error)
            else if (reloadRequired) Text(tr("Перед следующей заменой перечитайте файл. Черновик можно сохранить копией.",
                "Reload before replacing the file again. The draft can be saved as a copy."))
            SecretInput { OutlinedTextField(text, { value -> if (value.length <= 65_536) { text = value; guard.dirty = changed || value != opened?.text; copySaved = null; saved = false } },
                readOnly = opened == null || !writable || guard.busy || session.protectedWrite.phase != ProtectedWritePhase.Ready,
                modifier = Modifier.weight(1f).fillMaxWidth(), label = { Text(tr("Текст", "Text")) },
                textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace)) }
            Button(enabled = opened != null && writable && !guard.busy && guard.dirty && !changed && !reloadRequired &&
                session.protectedWrite.phase == ProtectedWritePhase.Ready, onClick = {
                val original = document ?: return@Button
                val submitted = text
                busy(true); fault = null; saved = false; copySaved = null
                scope.launch {
                    try {
                        val result = protectedWrite(session) { gateway.textSave(handle, path, entry.name, original.version, submitted) }
                        currentCoroutineContext().ensureActive()
                        when (result) {
                            is CoreResult.Value -> {
                                saved = true; guard.dirty = false; reloadRequired = true
                                // Save succeeded even if the following read fails: compare
                                // subsequent edits against these saved bytes, not the old text.
                                // The old token stays unusable until refresh succeeds.
                                document = original.copy(text = submitted)
                                // The save returns Unit: acquire a new token without ever binding
                                // this draft to somebody else's intervening edit.
                                when (val refreshed = gateway.textOpen(handle, path, entry.name)) {
                                    is CoreResult.Value -> {
                                        currentCoroutineContext().ensureActive()
                                        if (refreshed.value.text == submitted) { document = refreshed.value; reloadRequired = false }
                                        else { changed = true; guard.dirty = true }
                                    }
                                    is CoreResult.Failure -> fault = refreshed.fault
                                }
                            }
                            is CoreResult.Failure -> {
                                fault = result.fault; changed = result.fault == CoreFault.Changed
                                guard.uncertain = result.fault.hasUnknownWriteOutcome()
                            }
                        }
                    } catch (cancelled: CancellationException) { throw cancelled }
                    catch (_: Exception) { fault = CoreFault.Unavailable; reloadRequired = true }
                    finally { busy(false) }
                }
            }) { Text(tr("Сохранить в исходный файл", "Save original")) }
            Column(Modifier.heightIn(max = 160.dp).verticalScroll(rememberScrollState())) {
                OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
                    label = { Text(tr("Имя новой копии (.txt / .md)", "New copy name (.txt / .md)")) },
                    isError = name.isNotEmpty() && (!validNewTextName(name) || name == entry.name))
                Button(enabled = opened != null && writable && !guard.busy && validNewTextName(name) && name != entry.name &&
                    session.protectedWrite.phase == ProtectedWritePhase.Ready, onClick = {
                    val submitted = text; val target = name
                    busy(true); fault = null
                    scope.launch {
                        try {
                            val result = protectedWrite(session) { gateway.textCreate(handle, path, target, submitted) }
                            currentCoroutineContext().ensureActive()
                            when (result) {
                                is CoreResult.Value -> { copySaved = target; name = ""; guard.dirty = false }
                                is CoreResult.Failure -> { fault = result.fault; guard.uncertain = result.fault.hasUnknownWriteOutcome() }
                            }
                        } finally { busy(false) }
                    }
                }) { Text(tr("Сохранить копией", "Save a copy")) }
            }
        }
    }
}
