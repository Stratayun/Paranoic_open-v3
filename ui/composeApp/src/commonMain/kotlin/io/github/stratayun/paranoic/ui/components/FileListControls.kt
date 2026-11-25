package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

/** Adapted from v1.5: filters describe loaded rows, never a recursive search. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FileListControls(options: FileListOptions, view: FileView, visible: Int, loaded: Int,
    onOptions: (FileListOptions) -> Unit, onView: (FileView) -> Unit) {
    var menu by remember { mutableStateOf(false) }
    var searching by remember { mutableStateOf(options.query.isNotEmpty()) }
    val scheme = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (searching) OutlinedTextField(
            value = options.query,
            onValueChange = { if (it.length <= 256) onOptions(options.copy(query = it)) },
            modifier = Modifier.fillMaxWidth(), singleLine = true, shape = MaterialTheme.shapes.extraLarge,
            placeholder = { Text(tr("Поиск в этой папке", "Search this folder")) },
            leadingIcon = { AppIcon(AppSymbol.Search) },
            trailingIcon = {
                IconButton(onClick = { onOptions(options.copy(query = "")); searching = false },
                    modifier = Modifier.actionDescription(tr("Закрыть поиск", "Close search"))) { AppIcon(AppSymbol.Close) }
            },
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(if (visible == loaded) tr("Объектов: $loaded", "Items: $loaded") else tr("Показано: $visible из $loaded", "Showing: $visible of $loaded"),
                Modifier.weight(1f).padding(start = 8.dp), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            if (!searching) IconButton(onClick = { searching = true }, modifier = Modifier.actionDescription(tr("Поиск", "Search"))) { AppIcon(AppSymbol.Search) }
            Box {
                IconButton(onClick = { menu = true }, modifier = Modifier.actionDescription(tr("Сортировка: ", "Sort: ") + options.order.label())) { AppIcon(AppSymbol.Sort) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    FileOrder.entries.forEach { order ->
                        DropdownMenuItem(text = { Text(order.label()) }, trailingIcon = { if (order == options.order) AppIcon(AppSymbol.Forward) },
                            onClick = { menu = false; onOptions(options.copy(order = order)) })
                    }
                    HorizontalDivider()
                    DropdownMenuItem(text = { Text(tr("Показывать скрытые", "Show hidden")) },
                        trailingIcon = { Checkbox(checked = options.showHidden, onCheckedChange = null) },
                        onClick = { onOptions(options.copy(showHidden = !options.showHidden)) })
                }
            }
            val next = FileView.entries[(view.ordinal + 1) % FileView.entries.size]
            IconButton(onClick = { onView(next) }, modifier = Modifier.actionDescription(tr("Вид: ", "View: ") + next.fileViewLabel())) {
                AppIcon(if (view == FileView.Grid) AppSymbol.ViewList else AppSymbol.ViewGrid)
            }
        }
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
