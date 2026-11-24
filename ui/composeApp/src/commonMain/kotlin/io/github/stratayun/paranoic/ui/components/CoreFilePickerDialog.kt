package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.isActive

enum class CorePickKind { Files, FilesAndFolders, Directory }

/** A picker over Core identities only: no platform paths, grants or second IO engine. */
@Composable
fun CoreFilePickerDialog(core: CoreGateway, spaces: List<Space>, title: String, kind: CorePickKind,
    writable: Boolean = false, maxSelection: Int = 1, onDismiss: () -> Unit, onPick: (List<FileItem>) -> Unit) {
    require(maxSelection in 1..128)
    var trail by remember { mutableStateOf<List<FolderNode>>(emptyList()) }
    var listing by remember { mutableStateOf<FolderListing?>(null) }
    var listed by remember { mutableStateOf<Location?>(null) }
    var loading by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var selected by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var attempt by remember { mutableIntStateOf(0) }
    val folder = trail.lastOrNull()?.location
    LaunchedEffect(core, folder, attempt) {
        listing = null; listed = null; fault = null; loading = folder != null
        if (folder != null) try {
            val result = core.list(folder)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    currentCoroutineContext().ensureActive()
                    listing = result.value; listed = folder
                }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { if (currentCoroutineContext().isActive) loading = false }
    }
    val ready = !loading && listed == folder && listing != null && fault == null &&
        folder != null && spaces.any { it.id == folder.storageId && (!writable || it.writable) }
    val current = if (ready) listing else null
    val eligible = selected.isNotEmpty() && selected.all { item ->
        spaces.any { it.id == item.location.storageId && (!writable || it.writable) }
    }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Только уже подключённые папки. Исходные файлы сохраняются.", "Only connected folders are available. Source files are kept."),
                style = MaterialTheme.typography.bodySmall)
            Text(trail.joinToString(" / ") { displayName(it.name) }.ifEmpty { tr("Места", "Places") }, maxLines = 2)
            Row {
                TextButton(enabled = trail.isNotEmpty() && !loading, onClick = { trail = trail.dropLast(1) }) { Text(tr("Выше", "Up")) }
                TextButton(enabled = folder != null && !loading, onClick = { attempt++ }) { Text(tr("Обновить", "Refresh")) }
            }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it) }
            LazyColumn(Modifier.fillMaxWidth().heightIn(min = 100.dp, max = 240.dp)) {
                if (trail.isEmpty()) {
                    items(spaces.filter { !writable || it.writable }, key = { it.id }) { space ->
                        TextButton(onClick = { trail = listOf(FolderNode(space.root, space.name)) }) { Text(displayName(space.name)) }
                    }
                    if (spaces.none { !writable || it.writable }) item { Text(tr("Нет подходящих подключённых папок.", "No suitable connected folders.")) }
                } else {
                    items(current?.items.orEmpty().filter { it.kind == EntryKind.Directory ||
                        kind != CorePickKind.Directory && it.kind == EntryKind.File },
                        key = { it.location.storageId + ":" + it.location.path }) { item ->
                        Column(Modifier.fillMaxWidth()) {
                            if (item.kind == EntryKind.Directory) TextButton(enabled = trail.size < 64,
                                onClick = { trail = trail + FolderNode(item.location, item.name) }) {
                                Text(tr("Открыть папку: ", "Open folder: ") + displayName(item.name))
                            }
                            if (kind != CorePickKind.Directory && (item.kind == EntryKind.File || kind == CorePickKind.FilesAndFolders)) {
                                val checked = selected.any { it.location == item.location }
                                LabeledCheckbox(displayName(item.name), checked,
                                    enabled = checked || maxSelection == 1 || selected.size < maxSelection, onChange = { choose ->
                                        selected = if (choose) {
                                            if (maxSelection == 1) listOf(item) else selected + item
                                        } else selected.filterNot { it.location == item.location }
                                    })
                            }
                        }
                    }
                    if (current != null && current.items.isEmpty()) item { Text(tr("Папка пуста.", "The folder is empty.")) }
                }
            }
            if ((current?.hidden ?: 0) > 0) Text(tr("Некоторые служебные или непереносимые имена скрыты Core.",
                "Core hides some service or non-portable names."), style = MaterialTheme.typography.bodySmall)
            if (kind != CorePickKind.Directory) Row {
                Text(tr("Выбрано: ${selected.size} / $maxSelection", "Selected: ${selected.size} / $maxSelection"))
                TextButton(enabled = selected.isNotEmpty(), onClick = { selected = emptyList() }) { Text(tr("Сбросить", "Reset")) }
            }
        } },
        confirmButton = { TextButton(enabled = core.connected && if (kind == CorePickKind.Directory) ready else eligible && !loading,
            onClick = {
                if (kind == CorePickKind.Directory) {
                    val node = trail.lastOrNull()
                    if (ready && node != null) onPick(listOf(FileItem(node.location, node.name, EntryKind.Directory, 0uL)))
                } else if (eligible && !loading) onPick(selected.toList())
            }) { Text(if (kind == CorePickKind.Directory) tr("Выбрать эту папку", "Choose this folder") else tr("Выбрать", "Choose")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
