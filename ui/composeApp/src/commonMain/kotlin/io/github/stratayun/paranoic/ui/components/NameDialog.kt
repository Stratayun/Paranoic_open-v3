package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.plausibleOperationName
import io.github.stratayun.paranoic.ui.state.nameErrorText
import io.github.stratayun.paranoic.ui.state.fieldError

/** Preliminary UX check only. Rust ResourcePath and the storage enforce all rules. */
fun plausibleName(name: String): Boolean = plausibleOperationName(name)

/** Replaces v1.5 operation-controller coupling with an explicit command callback. */
@Composable
fun CreateDirectoryDialog(onDismiss: () -> Unit, onCreate: (String) -> Unit) {
    var name by remember { mutableStateOf("") }
    val invalid = name.isNotEmpty() && !plausibleName(name)
    val error = nameErrorText()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(tr("Новая папка", "New folder")) },
        text = {
            Column(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(value = name, onValueChange = { if (it.length <= 255) name = it },
                    modifier = Modifier.fillMaxWidth().fieldError(error, invalid), singleLine = true,
                    label = { Text(tr("Имя", "Name")) }, isError = invalid,
                    supportingText = { if (invalid) Text(error) })
                Text(tr("Без перезаписи существующего содержимого.", "Existing content will not be overwritten."))
            }
        },
        confirmButton = {
            TextButton(enabled = plausibleName(name), onClick = { onCreate(name) }) { Text(tr("Создать", "Create")) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } },
    )
}
