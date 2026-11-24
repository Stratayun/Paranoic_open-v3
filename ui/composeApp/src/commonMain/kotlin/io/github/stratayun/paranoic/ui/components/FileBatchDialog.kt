package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.selection.toggleable
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

/** Reviews the entire frozen selection. No write takes place while browsing. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FileBatchDialog(core: CoreGateway, spaces: List<Space>, sources: List<FileItem>, kind: FileBatchKind,
    onDismiss: () -> Unit, onSubmit: (FileBatchPlan) -> Unit) {
    var trail by remember(sources, kind) { mutableStateOf<List<FolderNode>>(emptyList()) }
    var entries by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var listed by remember { mutableStateOf<Location?>(null) }
    var loading by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var hidden by remember { mutableStateOf(0L) }
    var revision by remember { mutableIntStateOf(0) }
    var conflict by remember { mutableStateOf(NameConflict.Fail) }
    var recursive by remember { mutableStateOf(false) }
    val parent = trail.lastOrNull()?.location
    val deleting = kind == FileBatchKind.Delete
    LaunchedEffect(core, parent, revision) {
        listed = null; entries = emptyList(); fault = null; hidden = 0
        loading = parent != null
        if (parent != null) {
            try {
                val result = core.list(parent)
                currentCoroutineContext().ensureActive()
                when (result) {
                    is CoreResult.Value -> { entries = result.value.items; hidden = result.value.hidden; listed = parent }
                    is CoreResult.Failure -> fault = result.fault
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { fault = CoreFault.Unavailable }
            finally { loading = false }
        }
    }
    val plan = if (deleting || parent != null) FileBatchPlan(kind, sources, if (deleting) null else parent, conflict, recursive) else null
    val colliding = !deleting && listed == parent && sources.any { source -> entries.any { it.name == source.name } }
    val ready = core.connected && plan?.allowed(spaces) == true &&
        (deleting || (!loading && fault == null && listed == parent && !(colliding && conflict == NameConflict.Fail)))
    AlertDialog(onDismissRequest = onDismiss,
        title = { Text(when (kind) {
            FileBatchKind.Copy -> tr("Копировать выбранное", "Copy selected")
            FileBatchKind.Move -> tr("Перенести выбранное", "Move selected")
            FileBatchKind.Delete -> tr("Удалить выбранное", "Delete selected")
        }) },
        text = { Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Объектов: ${sources.size}. Проверьте весь список.", "Entries: ${sources.size}. Review the entire list."))
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 120.dp)) {
                items(sources, key = { it.location.storageId + ":" + it.location.path }) { source -> Text(displayName(source.name)) }
            }
            if (deleting) {
                Text(tr("Локальное удаление необратимо. Восстановление у облачного провайдера не гарантируется.",
                    "Local deletion is permanent. Recovery by a cloud provider is not guaranteed."))
                if (sources.any { it.kind == EntryKind.Directory }) {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline()
                        .toggleable(value = recursive, role = Role.Checkbox, onValueChange = { recursive = it }),
                        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Checkbox(recursive, null)
                        Text(tr("Удалить содержимое папок", "Delete folder contents"), Modifier.weight(1f))
                    }
                    Text(if (recursive) tr("Выбранные папки удалятся со всем содержимым.", "Selected folders and all their contents will be deleted.")
                        else tr("Непустые папки будут отклонены. Файлы всё равно будут удалены.", "Non-empty folders will be refused. Files will still be deleted."))
                }
            } else {
                Text(tr("Имена сохраняются. Папки не объединяются, замена существующих файлов не выполняется.",
                    "Names are preserved. Folders are not merged and existing files are not replaced."))
                FilesNavigation(trail) { trail = trail.take(it + 1) }
                TextButton(onClick = { trail = emptyList() }) { Text(tr("Все места", "All locations")) }
                if (loading) { LinearProgressIndicator(Modifier.fillMaxWidth()); Text(tr("Читаем папку назначения…", "Reading destination folder…")) }
                fault?.let { FaultNotice(it)
                    TextButton(onClick = { revision++ }) { Text(tr("Повторить чтение", "Read again")) } }
                LazyColumn(Modifier.fillMaxWidth().height(160.dp)) {
                    if (parent == null && spaces.none { it.writable }) item { Text(tr("Нет доступных мест для записи.", "No writable locations available.")) }
                    if (parent == null) items(spaces.filter { it.writable }, key = { it.id }) { space ->
                        TextButton(onClick = { trail = listOf(FolderNode(space.root, space.name)) }) { Text(displayName(space.name)) }
                    } else items(entries.filter { it.kind == EntryKind.Directory }, key = { it.location.path }) { folder ->
                        TextButton(onClick = { trail = trail + FolderNode(folder.location, folder.name) }) { Text(displayName(folder.name)) }
                    }
                }
                if (kind == FileBatchKind.Copy) {
                    Text(tr("При совпадении имени", "If a name exists"))
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        listOf(NameConflict.Fail, NameConflict.Skip, NameConflict.KeepBoth).forEach { choice ->
                            FilterChip(selected = conflict == choice, onClick = { conflict = choice }, label = { Text(when (choice) {
                                NameConflict.Fail -> tr("Отказ", "Fail")
                                NameConflict.Skip -> tr("Пропустить", "Skip")
                                else -> tr("Сохранить оба", "Keep both")
                            }) })
                        }
                    }
                }
                if (colliding && conflict == NameConflict.Fail) Text(tr("Есть занятые имена. Выберите другую папку или поведение при конфликте.",
                    "Some names already exist. Choose another folder or conflict policy."), color = MaterialTheme.colorScheme.error)
                if (parent != null && plan?.allowed(spaces) != true) Text(tr("Эта папка подходит не всем объектам. Проверьте права, вложенность и ограничения подключения.",
                    "This folder is not valid for every entry. Check permissions, nesting and connection restrictions."))
                if (hidden > 0) Text(tr("Часть имён скрыта ядром; дополнительные конфликты проверит ядро.", "Core hid some names and will check additional conflicts."))
            }
            Text(tr("Каждый объект отправляется отдельным заданием. При отказе возможен частичный результат; неизвестный ответ останавливает оставшуюся отправку. Автоматического повтора нет.",
                "Each entry is a separate job. Failures can leave partial results; an unknown reply stops remaining submissions. There is no automatic retry."))
        } },
        confirmButton = { TextButton(enabled = ready, onClick = { plan?.let(onSubmit) }) {
            Text(if (deleting) tr("Удалить ${sources.size}", "Delete ${sources.size}") else tr("Отправить ${sources.size} заданий", "Submit ${sources.size} jobs"),
                color = if (deleting) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary)
        } }, dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
