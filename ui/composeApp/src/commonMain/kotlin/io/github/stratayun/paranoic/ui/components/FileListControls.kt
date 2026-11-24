package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

/** Adapted from v1.5: filters describe loaded rows, never a recursive search. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FileListControls(options: FileListOptions, view: FileView, visible: Int, loaded: Int,
    onOptions: (FileListOptions) -> Unit, onView: (FileView) -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            value = options.query,
            onValueChange = { if (it.length <= 256) onOptions(options.copy(query = it)) },
            modifier = Modifier.fillMaxWidth(), singleLine = true,
            label = { Text(tr("Поиск в загруженном списке", "Search loaded entries")) },
            trailingIcon = {
                if (options.query.isNotEmpty()) TextButton(onClick = { onOptions(options.copy(query = "")) }) {
                    Text(tr("Сброс", "Clear"))
                }
            },
        )
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box {
                TextButton(onClick = { menu = true }) { Text(options.order.label()) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    FileOrder.entries.forEach { order ->
                        DropdownMenuItem(text = { Text(order.label()) },
                            onClick = { menu = false; onOptions(options.copy(order = order)) })
                    }
                }
            }
            FilterChip(selected = options.showHidden,
                onClick = { onOptions(options.copy(showHidden = !options.showHidden)) },
                label = { Text(tr("Скрытые", "Hidden")) })
            FileView.entries.forEach { choice ->
                FilterChip(selected = view == choice, onClick = { onView(choice) }, label = { Text(choice.fileViewLabel()) })
            }
        }
        Text(tr("Показано: $visible из $loaded", "Showing: $visible of $loaded"),
            style = MaterialTheme.typography.labelMedium)
    }
}
@Composable
fun FileView.fileViewLabel() = when (this) {
    FileView.List -> tr("Список", "List")
    FileView.Detailed -> tr("Подробно", "Details")
    FileView.Grid -> tr("Плитки", "Grid")
}
@Composable
private fun FileOrder.label() = when (this) {
    FileOrder.NameAscending -> tr("Имя А → Я", "Name A → Z")
    FileOrder.NameDescending -> tr("Имя Я → А", "Name Z → A")
    FileOrder.SizeAscending -> tr("Размер: по возрастанию", "Size: ascending")
    FileOrder.SizeDescending -> tr("Размер: по убыванию", "Size: descending")
}
