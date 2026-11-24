package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

/** Creates a new empty UTF-8 document through Core, without replacing a file. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun CreateTextDialog(existingNames: Set<String>, onDismiss: () -> Unit, onCreate: (String) -> Unit) {
    val defaultName = tr("Новый файл", "New file")
    var name by remember { mutableStateOf("$defaultName.txt") }
    val valid = validNewTextName(name)
    val exists = name in existingNames
    AlertDialog(onDismissRequest = onDismiss, title = { Text(tr("Новый текстовый файл", "New text file")) },
        text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("txt", "md").forEach { extension ->
                    FilterChip(selected = name.endsWith(".$extension", true), onClick = {
                        name = name.substringBeforeLast('.', name) + ".$extension"
                    }, label = { Text(if (extension == "txt") tr("Текст .txt", "Text .txt") else "Markdown .md") })
                }
            }
            OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
                label = { Text(tr("Имя файла", "File name")) }, isError = !valid || exists,
                supportingText = { if (!valid) Text(tr("Нужно допустимое имя с расширением .txt или .md.", "Enter a valid name ending in .txt or .md."))
                    else if (exists) Text(tr("Такое имя уже занято.", "This name already exists.")) })
            Text(tr("В текущей папке появится пустой файл UTF-8. Существующие файлы не заменяются.",
                "An empty UTF-8 file is created in the current folder. Existing files are not replaced."))
        } }, confirmButton = { TextButton(enabled = valid && !exists, onClick = { onCreate(name) }) { Text(tr("Создать", "Create")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
