package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun RenameEntryDialog(source: FileItem, onDismiss: () -> Unit, onRename: (String) -> Unit) {
    var name by remember(source) { mutableStateOf(source.name) }
    val invalid = name.isNotEmpty() && !plausibleName(name)
    val error = nameErrorText()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(tr("Переименовать", "Rename")) },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Текущее имя: ", "Current name: ") + displayName(source.name))
                OutlinedTextField(value = name, onValueChange = { if (it.length <= 255) name = it },
                    modifier = Modifier.fillMaxWidth().fieldError(error, invalid), singleLine = true,
                    label = { Text(tr("Новое имя", "New name")) },
                    isError = invalid, supportingText = { if (invalid) Text(error) })
                Text(tr("В той же папке, без замены существующего объекта.",
                    "In the same folder, without replacing an existing entry."))
            }
        },
        confirmButton = {
            TextButton(enabled = plausibleName(name) && name != source.name, onClick = { onRename(name) }) {
                Text(tr("Переименовать", "Rename"))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } },
    )
}

@Composable
fun DeleteEntryDialog(source: FileItem, onDismiss: () -> Unit, onDelete: (Boolean) -> Unit) {
    var recursive by remember(source) { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(tr("Подтвердить удаление", "Confirm deletion")) },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(source.name))
                Text(tr("Локальное удаление необратимо. Для облака способ удаления задаёт провайдер; восстановление не гарантируется.",
                    "Local deletion is permanent. Cloud deletion follows the provider's behavior; recovery is not guaranteed."))
                if (source.kind == EntryKind.Directory) {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().toggleable(value = recursive, role = Role.Checkbox,
                        onValueChange = { recursive = it })) {
                        Checkbox(checked = recursive, onCheckedChange = null)
                        Text(tr("Удалить папку со всем содержимым", "Delete folder and all its contents"),
                            Modifier.padding(top = 12.dp).weight(1f))
                    }
                    Text(if (recursive) tr("Будут удалены и вложенные файлы и папки.", "Nested files and folders will also be deleted.")
                        else tr("Без этой отметки удалится только пустая папка. Непустая папка даст отказ.",
                            "Without this option only an empty folder can be deleted. A non-empty folder will be refused."))
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onDelete(recursive) }) {
                Text(if (recursive) tr("Удалить со всем содержимым", "Delete with all contents")
                    else tr("Удалить", "Delete"), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } },
    )
}

/** Separate confirmation: selecting the Replace policy never dispatches a write. */
@Composable
fun ReplaceFileDialog(source: FileItem, target: FileItem, destination: List<FolderNode>,
    onDismiss: () -> Unit, onReplace: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(tr("Заменить существующий файл?", "Replace the existing file?")) },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Копия из: ", "Copy from: ") + displayName(source.name))
                Text(tr("Папка назначения: ", "Destination folder: ") +
                    destination.joinToString(" / ") { displayName(it.name) })
                Text(tr("Заменяемый файл: ", "File to replace: ") + displayName(target.name))
                Text(tr("Прежнее содержимое этого файла будет заменено. Core повторно проверит допустимость операции. Отмена этого диалога ничего не записывает.",
                    "The previous file contents will be replaced. Core rechecks whether the operation is supported. Cancelling this dialog writes nothing."))
            }
        },
        confirmButton = {
            TextButton(onClick = onReplace) { Text(tr("Заменить файл", "Replace file"), color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } },
    )
}
