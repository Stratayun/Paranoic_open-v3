package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*

enum class ArchiveAction { Open, Create, Extract }

@Composable
@OptIn(ExperimentalLayoutApi::class)
fun ArchiveDialog(session: AppSession, sources: List<FileItem>, action: ArchiveAction,
    onDismiss: () -> Unit, onOpened: () -> Unit = {}) {
    val source = sources.firstOrNull() ?: return
    val archiveName = tr("архив", "archive")
    val extractedName = tr("распаковано", "extracted")
    var format by remember(sources, action) { mutableStateOf(if (action == ArchiveAction.Extract) session.archives.state.format
        else if (source.name.endsWith(".7z", true)) ArchiveFormat.SevenZip else ArchiveFormat.Zip) }
    var name by remember(sources, action) { mutableStateOf(if (action == ArchiveAction.Create) archiveName + "." + format.extension
        else source.name.substringBeforeLast('.', source.name).ifBlank { extractedName }) }
    var parent by remember { mutableStateOf(session.files.folder) }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    var protected by remember { mutableStateOf(false) }
    val passwordAssessment = rememberPasswordAssessment(session.core, if (action == ArchiveAction.Create && protected) password else "")
    var choosing by remember { mutableStateOf(false) }
    DisposableEffect(Unit) { onDispose { password = ""; repeat = "" } }
    val output = action != ArchiveAction.Open
    val to = parent
    val collision = output && to == session.files.folder && session.files.entries.any { it.name == name }
    val passwordValid = action != ArchiveAction.Create || !protected || (format == ArchiveFormat.Zip && passwordAssessment.acceptsPassword() && password == repeat)
    val valid = session.core.connected && !session.writing && passwordValid &&
        (!output || to != null && session.files.spaces.spaceOf(to)?.writable == true && plausibleName(name) && !collision) &&
        session.directWrite?.phase != DirectWritePhase.Unknown && session.uncertain == null && session.uncertainAttention == null
    fun dismiss() { password = ""; repeat = ""; onDismiss() }
    val title = when (action) {
        ArchiveAction.Open -> tr("Открыть архив", "Open archive")
        ArchiveAction.Create -> tr("Создать архив", "Create archive")
        ArchiveAction.Extract -> tr("Распаковать в новую папку", "Extract to a new folder")
    }
    AlertDialog(onDismissRequest = ::dismiss, title = { Text(title) }, text = {
        Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (action == ArchiveAction.Create && protected) {
                PasswordTools(session.core, password, assessment = passwordAssessment) { password = it; repeat = "" }
            }
            Text(if (action == ArchiveAction.Create) tr("Выбрано объектов: ${sources.size}", "Selected entries: ${sources.size}") else displayName(source.name))
            if (action == ArchiveAction.Open && session.archives.state.source?.location == source.location)
                session.archives.state.fault?.let { FaultNotice(it) }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ArchiveFormat.entries.forEach { f -> FilterChip(selected = format == f, enabled = action != ArchiveAction.Extract,
                    onClick = { format = f; password = ""; repeat = ""; protected = false; if (action == ArchiveAction.Create) name = archiveName + "." + f.extension },
                    label = { Text(if (f == ArchiveFormat.Zip) "ZIP / ZIP64" else "7z") }) }
            }
            Text(tr("Формат выбирается явно; расширение только подсказывает выбор.", "Choose the format explicitly; the extension is only a hint."))
            if (action == ArchiveAction.Create) {
                Text(tr("Файлы и папки добавляются через Core; исходники сохраняются. До 10 000 записей. Существующее имя не заменяется.",
                    "Core adds files and folders; sources are kept. Up to 10,000 entries. Existing names are not replaced."))
                if (format == ArchiveFormat.Zip) LabeledCheckbox(tr("Защитить ZIP паролем (AES)", "Password-protect ZIP (AES)"),
                    checked = protected, onChange = { protected = it; password = ""; repeat = "" })
                else Text(tr("Создание 7z сейчас только без пароля. Зашифрованные 7z можно читать и распаковывать.",
                    "7z creation is currently unencrypted. Encrypted 7z archives can be read and extracted."))
            }
            if (action != ArchiveAction.Create || protected) {
                OutlinedTextField(password, { if (it.length <= 4096) password = it }, singleLine = true,
                    label = { Text(if (action == ArchiveAction.Create) tr("Пароль", "Password") else tr("Пароль (если нужен)", "Password (if required)")) },
                    visualTransformation = peek("ArchiveDialog:4"), trailingIcon = { PeekToggle("ArchiveDialog:4") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false))
                if (action == ArchiveAction.Create) {
                    OutlinedTextField(repeat, { if (it.length <= 4096) repeat = it }, singleLine = true, label = { Text(tr("Повторите пароль", "Repeat password")) },
                        modifier = Modifier.fieldError(passwordMismatchText(), repeat.isNotEmpty() && repeat != password),
                        visualTransformation = peek("ArchiveDialog:5"), trailingIcon = { PeekToggle("ArchiveDialog:5") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                        isError = repeat.isNotEmpty() && repeat != password, supportingText = { if (repeat.isNotEmpty() && repeat != password) Text(passwordMismatchText()) })
                    Text(tr("Минимум 8 символов: строчные и заглавные буквы, цифра и символ.", "At least 8 characters: lower- and uppercase letters, a digit and a symbol."))
                }
            }
            if (format == ArchiveFormat.SevenZip && action != ArchiveAction.Create)
                Text(tr("Декодер 7z запускается отдельно. Если компонент ещё не поставлен в сборку, операция будет недоступна.",
                    "7z decoding runs in a separate process. If the component is not packaged yet, the operation is unavailable."))
            if (output) {
                OutlinedTextField(name, { if (it.length <= 1024) name = it }, singleLine = true,
                    modifier = Modifier.fieldError(if (collision) tr("Имя уже занято.", "Name already exists.") else nameErrorText(), !plausibleName(name) || collision),
                    label = { Text(if (action == ArchiveAction.Extract) tr("Имя новой папки", "New folder name") else tr("Имя архива", "Archive name")) },
                    isError = !plausibleName(name) || collision, supportingText = { if (!plausibleName(name)) Text(nameErrorText()) })
                to?.let { Text(tr("Назначение: ", "Destination: ") + displayName(session.files.spaces.spaceOf(it)?.name ?: it.storageId) + ": /" + displayName(it.path)) }
                TextButton(onClick = { choosing = true }) { Text(tr("Выбрать другую папку…", "Choose another folder…")) }
                if (collision) Text(tr("Имя занято; выберите другое. Слияния и замены нет.", "Name already exists; choose another. No merging or replacement."), color = MaterialTheme.colorScheme.error)
                if (action == ArchiveAction.Extract) Text(tr("Содержимое публикуется только после распаковки целиком, затем перечитывается и проверяется. Зашифрованный архив даст незашифрованные файлы в выбранной папке.",
                    "Contents are published only after full extraction, then reread and verified. An encrypted archive produces unencrypted files in the chosen folder."))
            }
        }
    }, confirmButton = { TextButton(enabled = valid, onClick = {
        val secret = password.takeIf { it.isNotEmpty() && (action != ArchiveAction.Create || protected) }
        password = ""; repeat = ""
        when (action) {
            ArchiveAction.Open -> {
                if (session.archives.state.source == source) { session.archives.open(source, format, secret); onOpened() }
                else if (session.openArchive(source, format, secret)) onOpened()
            }
            ArchiveAction.Create -> to?.let { session.createArchive(sources, format, secret, it, name) }
            ArchiveAction.Extract -> to?.let { session.extractArchive(source, format, secret, it, name) }
        }
        onDismiss()
    }) { Text(title) } }, dismissButton = { TextButton(onClick = ::dismiss) { Text(tr("Отмена", "Cancel")) } })
    if (choosing) TransferDestinationDialog(session.core, source, session.files.spaces, TransferMode.Copy,
        onDismiss = { choosing = false }, onTransfer = { destination, outputName, _, _ -> parent = destination; name = outputName; choosing = false },
        conflictChoices = false, initialName = name, selectionOnly = true)
}
