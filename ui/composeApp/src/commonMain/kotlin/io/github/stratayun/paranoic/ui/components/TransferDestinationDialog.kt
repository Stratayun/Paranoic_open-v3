package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.isActive

/** Browsing and policies are presentation only; Core performs every file operation. */
@Composable
fun TransferDestinationDialog(core: CoreGateway, source: FileItem, spaces: List<Space>, mode: TransferMode,
    onDismiss: () -> Unit, onTransfer: (Location, String, NameConflict, FileItem?) -> Unit,
    conflictChoices: Boolean = true, initialName: String = source.name, selectionOnly: Boolean = false) {
    var trail by remember(source.location, mode) { mutableStateOf<List<FolderNode>>(emptyList()) }
    var name by remember(source.location, mode, initialName) { mutableStateOf(initialName) }
    var entries by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var loading by remember { mutableStateOf(false) }
    var loaded by remember { mutableStateOf(false) }
    var listedParent by remember { mutableStateOf<Location?>(null) }
    var hidden by remember { mutableStateOf(0L) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    val parent = trail.lastOrNull()?.location
    var conflict by remember(parent, name, mode) { mutableStateOf(NameConflict.Fail) }
    var replacing by remember(parent, name, mode) { mutableStateOf<FileItem?>(null) }
    LaunchedEffect(core, parent) {
        entries = emptyList()
        loaded = false
        listedParent = null
        hidden = 0
        fault = null
        loading = parent != null
        if (parent != null) {
            try {
                val result = core.list(parent)
                currentCoroutineContext().ensureActive()
                when (result) {
                    is CoreResult.Value -> {
                        entries = result.value.items
                        hidden = result.value.hidden
                        loaded = true
                        listedParent = parent
                    }
                    is CoreResult.Failure -> fault = result.fault
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { fault = CoreFault.Unavailable }
            finally { if (currentCoroutineContext().isActive) loading = false }
        }
    }
    val currentEntries = if (listedParent == parent) entries else emptyList()
    val collision = currentEntries.singleOrNull { it.name == name }
    val allowedParent = parent != null && source.canTransferTo(parent, mode, spaces)
    val replaceAllowed = mode == TransferMode.Copy && parent != null && source.canReplace(collision, parent, name, spaces)
    val sameMove = mode == TransferMode.Move && parent != null && source.isTarget(parent, name)
    val invalidName = name.isNotEmpty() && !plausibleName(name)
    val nameError = nameErrorText()
    val ready = core.connected && parent != null && listedParent == parent && loaded && !loading && fault == null && plausibleName(name) &&
        allowedParent && !sameMove && (collision == null || conflict != NameConflict.Fail) &&
        (conflict != NameConflict.Replace || replaceAllowed)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (selectionOnly) tr("Выбрать назначение", "Choose destination")
            else if (mode == TransferMode.Copy) tr("Копировать в папку", "Copy to folder")
            else tr("Перенести в папку", "Move to folder")) },
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(source.name), maxLines = 2)
                OutlinedTextField(value = name, onValueChange = { if (it.length <= 255) name = it },
                    modifier = Modifier.fillMaxWidth().fieldError(nameError, invalidName), singleLine = true,
                    label = { Text(tr("Новое имя", "Destination name")) },
                    isError = invalidName, supportingText = { if (invalidName) Text(nameError) })
                FilesNavigation(trail) { trail = trail.take(it + 1) }
                TextButton(onClick = { trail = emptyList() }) { Text(tr("Пространства", "Spaces")) }
                if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
                fault?.let { FaultNotice(it) }
                LazyColumn(Modifier.fillMaxWidth().height(180.dp)) {
                    if (trail.isEmpty()) {
                        items(spaces.filter { it.writable }, key = { it.id }) { space ->
                            TextButton(onClick = { trail = listOf(FolderNode(space.root, space.name)) }) {
                                Text(displayName(space.name))
                            }
                        }
                        if (spaces.none { it.writable }) item {
                            Text(tr("Нет доступных папок для записи.", "No writable folders available."))
                        }
                    } else {
                        items(currentEntries.filter { it.kind == EntryKind.Directory },
                            key = { it.location.storageId + ":" + it.location.path }) { folder ->
                            TextButton(onClick = { trail = trail + FolderNode(folder.location, folder.name) }) {
                                AppIcon(AppSymbol.Folder)
                                Text(displayName(folder.name), Modifier.padding(start = 8.dp))
                            }
                        }
                    }
                }
                if (parent != null && !allowedParent) Text(tr(
                    "Эта папка недоступна для операции: проверьте права, вложенность и ограничения подключения.",
                    "This destination is unavailable: check permissions, nesting and connection restrictions."))
                if (sameMove) Text(tr("Выберите другую папку или другое имя.", "Choose another folder or name."))
                collision?.let { Text(tr("Имя уже занято: ", "Name already exists: ") + displayName(it.name)) }
                if (hidden > 0) Text(tr("Часть имён скрыта Core; возможен дополнительный конфликт.",
                    "Some names are hidden by Core; an additional conflict is possible."))
                if (mode == TransferMode.Copy && conflictChoices) {
                    Text(tr("При совпадении имени", "If the name exists"))
                    val choices = listOf(NameConflict.Fail, NameConflict.Skip, NameConflict.KeepBoth) +
                        if (replaceAllowed) listOf(NameConflict.Replace) else emptyList()
                    choices.forEach { policy ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().selectable(selected = conflict == policy, role = Role.RadioButton,
                            onClick = { conflict = policy; replacing = null })) {
                            RadioButton(selected = conflict == policy, onClick = null)
                            Text(policy.conflictLabel(), Modifier.padding(top = 12.dp))
                        }
                    }
                    Text(when (conflict) {
                        NameConflict.Fail -> tr("Существующее имя не перезаписывается. При конфликте измените имя или поведение.",
                            "Existing names are not overwritten. Change the name or policy if a conflict occurs.")
                        NameConflict.Skip -> tr("Если имя занято, объект пропускается целиком. Папки не объединяются.",
                            "If the name exists, the whole entry is skipped. Folders are not merged.")
                        NameConflict.KeepBoth -> tr("Core подберёт свободное имя для копии. Папки не объединяются.",
                            "Core chooses an available name for the copy. Folders are not merged.")
                        NameConflict.Replace -> tr("Только файл поверх файла, после отдельного подтверждения.",
                            "File over file only, with a separate confirmation.")
                    })
                } else Text(tr("Без перезаписи. При совпадении выберите другое имя или папку.",
                    "No overwriting. Choose another name or folder if it already exists."))
                if (mode == TransferMode.Move) Text(tr(
                    "Между пространствами: сначала проверенная копия, затем удаление исходника.",
                    "Between spaces: verified copy, then source deletion."))
                if (mode == TransferMode.Copy && conflictChoices) Text(tr(
                    "Журнал покажет пропуск или результат, включая новое имя при сохранении обоих.",
                    "The journal shows skips and results, including the new name when keeping both."))
            }
        },
        confirmButton = {
            TextButton(enabled = ready, onClick = {
                val destination = parent ?: return@TextButton
                if (conflict == NameConflict.Replace) replacing = collision
                else onTransfer(destination, name, conflict, null)
            }) { Text(if (selectionOnly) tr("Выбрать папку и имя", "Choose folder and name")
                else if (conflict == NameConflict.Replace) tr("Подтвердить замену…", "Confirm replacement…")
                else if (mode == TransferMode.Copy) tr("Копировать сюда", "Copy here") else tr("Перенести сюда", "Move here")) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } },
    )
    replacing?.let { target ->
        ReplaceFileDialog(source, target, trail, onDismiss = { replacing = null }) {
            val destination = parent
            if (ready && destination != null && conflict == NameConflict.Replace && target == collision &&
                source.canReplace(target, destination, name, spaces)) {
                replacing = null
                onTransfer(destination, name, NameConflict.Replace, target)
            }
        }
    }
}

@Composable
private fun NameConflict.conflictLabel(): String = when (this) {
    NameConflict.Fail -> tr("Отказ при конфликте", "Fail on conflict")
    NameConflict.Skip -> tr("Пропустить", "Skip")
    NameConflict.KeepBoth -> tr("Сохранить оба", "Keep both")
    NameConflict.Replace -> tr("Заменить файл…", "Replace file…")
}
