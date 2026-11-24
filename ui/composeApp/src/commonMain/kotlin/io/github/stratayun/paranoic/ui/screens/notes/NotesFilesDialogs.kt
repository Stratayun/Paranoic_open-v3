package io.github.stratayun.paranoic.ui.screens.notes

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun NotesImportDialog(core: CoreGateway, spaces: List<Space>, tabs: List<NoteTab>,
    onDismiss: () -> Unit, onImport: (List<Location>, String, String?) -> Unit) {
    var choosing by remember { mutableStateOf(false) }
    var sources by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var title by remember { mutableStateOf("") }
    var target by remember { mutableStateOf<String?>(null) }
    val ready = sources.isNotEmpty() && title.isNotBlank() && title.length <= 128 &&
        (target == null || tabs.any { it.id == target })
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss,
        title = { Text(tr("Импорт заметок", "Import notes")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Файлы UTF-8 .txt/.md добавляются в тетрадь. Папка и её ближайшие подпапки дают отдельные вкладки; более глубокие деревья выбирайте отдельно. Исходники сохраняются, неподдерживаемые файлы пропускаются.",
                "UTF-8 .txt/.md files are added to the notebook. A folder and its immediate subfolders produce separate tabs; select deeper folders separately. Sources are kept and unsupported files are skipped."))
            Text(tr("Перед импортом текущие правки сохраняются. Повторный импорт создаёт дополнительные заметки.",
                "Current edits are saved before importing. Importing again creates additional notes."))
            Text(tr("Выбор папки вместе с файлами внутри неё может дать дубликаты. Для глубокой подпапки можно выполнить отдельный импорт.",
                "Selecting a folder together with files inside it can create duplicates. Import a deeper subfolder separately."), style = MaterialTheme.typography.bodySmall)
            TextButton(onClick = { choosing = true }) { Text(tr("Выбрать файлы / папки…", "Choose files / folders…")) }
            sources.take(8).forEach { Text(displayName(it.name), maxLines = 1) }
            if (sources.size > 8) Text(tr("И ещё ${sources.size - 8}", "And ${sources.size - 8} more"))
            OutlinedTextField(title, { if (it.length <= 128) title = it }, singleLine = true,
                label = { Text(tr("Название новой вкладки для отдельных файлов", "New tab title for loose files")) })
            Text(tr("Куда добавить", "Add to"))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                FilterChip(selected = target == null, onClick = { target = null }, label = { Text(tr("Новые вкладки", "New tabs")) })
                tabs.forEach { tab -> FilterChip(selected = target == tab.id, onClick = { target = tab.id },
                    label = { Text(displayName(tab.title), maxLines = 1) }) }
            }
        } },
        confirmButton = { TextButton(enabled = core.connected && ready,
            onClick = { if (ready) onImport(sources.map { it.location }, title, target) }) { Text(tr("Импортировать", "Import")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
    if (choosing) CoreFilePickerDialog(core, spaces, tr("Файлы для импорта", "Files to import"), CorePickKind.FilesAndFolders,
        maxSelection = 128, onDismiss = { choosing = false }, onPick = { picked ->
            sources = picked.distinctBy { it.location }
            choosing = false
        })
}

@Composable
internal fun NotesExportDialog(core: CoreGateway, spaces: List<Space>, onDismiss: () -> Unit,
    onExport: (Location, String) -> Unit) {
    var choosing by remember { mutableStateOf(false) }
    var parent by remember { mutableStateOf<FileItem?>(null) }
    var name by remember { mutableStateOf("") }
    var acknowledged by remember { mutableStateOf(false) }
    val destination = parent?.location
    val ready = destination != null && spaces.any { it.id == destination.storageId && it.writable } &&
        plausibleOperationName(name) && acknowledged
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss,
        title = { Text(tr("Экспорт открытых копий", "Export plaintext copies")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Все вкладки и заметки будут записаны как открытые .md в НОВУЮ папку. Вложения остаются внутри Сейфа и не экспортируются. Существующая папка не объединяется и не заменяется.",
                "All tabs and notes will be written as plaintext .md files in a NEW folder. Attachments remain inside the Safe and are not exported. An existing folder is never merged or replaced."))
            Text(tr("Эти копии доступны выбранному месту, другим приложениям и его резервным копиям. Clear, автостирание и пароль под принуждением их не удалят.",
                "These copies are accessible to the selected storage, other apps and its backups. Clear, auto-wipe and a duress password will not remove them."),
                color = MaterialTheme.colorScheme.error)
            TextButton(onClick = { choosing = true }) { Text(tr("Выбрать папку назначения…", "Choose destination folder…")) }
            parent?.let { Text(displayName(spaces.firstOrNull { space -> space.id == it.location.storageId }?.name.orEmpty()) +
                " / " + displayName(it.location.path), maxLines = 3) }
            OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
                label = { Text(tr("Имя новой папки", "New folder name")) },
                isError = name.isNotEmpty() && !plausibleOperationName(name))
            LabeledCheckbox(tr("Понимаю: копии не зашифрованы и не стираются вместе с Сейфом", "I understand: copies are unencrypted and are not wiped with the Safe"),
                acknowledged, onChange = { acknowledged = it })
        } },
        confirmButton = { TextButton(enabled = core.connected && ready, onClick = {
            if (ready && destination != null) onExport(destination, name)
        }) { Text(tr("Экспортировать", "Export")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
    if (choosing) CoreFilePickerDialog(core, spaces, tr("Назначение экспорта", "Export destination"), CorePickKind.Directory,
        writable = true, onDismiss = { choosing = false }, onPick = { parent = it.single(); choosing = false })
}
