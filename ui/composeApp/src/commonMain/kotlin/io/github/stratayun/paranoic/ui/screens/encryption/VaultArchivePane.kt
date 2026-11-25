package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun VaultArchivePane(session: AppSession, gateway: EncryptedFolderGateway, handle: String, path: List<String>,
    entry: VaultEntry, writable: Boolean, guard: DocumentExitGuard, onBusy: (Boolean) -> Unit, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var format by remember { mutableStateOf(if (entry.name.endsWith(".7z", true)) ArchiveFormat.SevenZip else ArchiveFormat.Zip) }
    var password by remember { mutableStateOf("") }
    var entries by remember { mutableStateOf<List<ArchiveEntry>?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var targetPath by remember { mutableStateOf(path) }
    var targetName by remember { mutableStateOf("") }
    var acknowledged by remember { mutableStateOf(false) }
    var choosing by remember { mutableStateOf(false) }
    var extracted by remember { mutableStateOf<ULong?>(null) }
    fun busy(value: Boolean) { guard.busy = value; onBusy(value) }
    suspend fun read(secret: String?) {
        entries = null; fault = null
        try {
            val result = gateway.archiveList(handle, path, entry.name, format, secret)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> entries = result.value
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
    }
    LaunchedEffect(Unit) { busy(true); try { read(null) } finally { busy(false) } }
    DisposableEffect(Unit) { onDispose { password = ""; entries = null; targetName = "" } }
    Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding().padding(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row {
            TextButton(enabled = !guard.busy, onClick = onClose) { Text(tr("Назад", "Back")) }
            Text(displayName(entry.name), Modifier.weight(1f).sectionHeading(), maxLines = 1)
        }
        Column(Modifier.fillMaxWidth().heightIn(max = 310.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                ArchiveFormat.entries.forEach { kind -> FilterChip(selected = format == kind, enabled = !guard.busy,
                    onClick = { format = kind; entries = null; password = ""; fault = null }, label = { Text(kind.extension) }) }
            }
            OutlinedTextField(password, { if (it.length <= 4096) password = it }, enabled = !guard.busy, singleLine = true,
                label = { Text(tr("Пароль архива, если нужен", "Archive password, if required")) },
                visualTransformation = peek("VaultArchivePane:20"), trailingIcon = { PeekToggle("VaultArchivePane:20") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
            Text(tr("Пароль не сохраняется: для каждой операции введите его заново.", "The password is not stored: enter it again for each operation."), style = MaterialTheme.typography.bodySmall)
            TextButton(enabled = !guard.busy, onClick = {
                val secret = password.takeIf { it.isNotEmpty() }; password = ""; busy(true)
                scope.launch { try { read(secret) } finally { busy(false) } }
            }) { Text(tr("Прочитать список", "Read entries")) }
            if (writable) {
                TextButton(enabled = !guard.busy, onClick = { choosing = true }) { Text(tr("Распаковать внутри: ", "Extract inside: ") + vaultPath(targetPath)) }
                OutlinedTextField(targetName, { if (it.length <= 255) targetName = it }, enabled = !guard.busy, singleLine = true,
                    label = { Text(tr("Имя новой зашифрованной папки", "New encrypted folder name")) },
                    isError = targetName.isNotEmpty() && !plausibleOperationName(targetName))
                Text(tr("Открытые файлы снаружи не создаются. При ошибке может остаться частично заполненная зашифрованная папка. Замены, слияния и автоматического повтора нет.",
                    "No plaintext files are created outside the vault. A failure can leave a partly filled encrypted folder. There is no replacement, merging or automatic retry."),
                    style = MaterialTheme.typography.bodySmall)
                LabeledCheckbox(tr("Подтверждаю распаковку в новую папку", "Extract into a new folder"), acknowledged, enabled = !guard.busy,
                    onChange = { acknowledged = it })
                Button(enabled = !guard.busy && entries != null && acknowledged && plausibleOperationName(targetName) &&
                    session.protectedWrite.phase == ProtectedWritePhase.Ready, onClick = {
                    val secret = password.takeIf { it.isNotEmpty() }; password = ""
                    val destination = targetPath.toList(); val name = targetName; val kind = format
                    busy(true); fault = null; extracted = null
                    scope.launch {
                        try {
                            val result = protectedWrite(session) { gateway.archiveExtract(handle, path, entry.name, kind, secret, destination, name) }
                            currentCoroutineContext().ensureActive()
                            when (result) {
                                is CoreResult.Value -> { extracted = result.value; targetName = ""; acknowledged = false }
                                is CoreResult.Failure -> { fault = result.fault; guard.uncertain = result.fault.hasUnknownWriteOutcome() }
                            }
                        } finally { busy(false) }
                    }
                }) { Text(tr("Распаковать в хранилище", "Extract into vault")) }
            }
            fault?.let { FaultNotice(it, message = if (it == CoreFault.WrongPassword && format == ArchiveFormat.SevenZip)
                tr("Неверный пароль или повреждённые зашифрованные данные 7z.", "Wrong password or damaged encrypted 7z data.") else it.label()) }
            extracted?.let { Text(tr("Зашифрованных файлов создано: $it", "Encrypted files created: $it")) }
        }
        if (guard.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        entries?.let { values ->
            Text(tr("Записей: ${values.size}", "Entries: ${values.size}"))
            LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                itemsIndexed(values, key = { index, _ -> index }) { _, member ->
                    Column(Modifier.padding(vertical = 6.dp)) {
                        Text(displayName(member.path), maxLines = 3)
                        Text(if (member.folder) tr("Папка", "Folder") else tr("${member.size} байт", "${member.size} bytes"), style = MaterialTheme.typography.bodySmall)
                        if (member.encrypted) Text(tr("Защищено паролем архива", "Protected by the archive password"), style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        } ?: Spacer(Modifier.weight(1f))
    }
    if (choosing) VaultFolderDialog(gateway, handle, targetPath, onDismiss = { choosing = false }) { chosen -> targetPath = chosen; choosing = false }
}
