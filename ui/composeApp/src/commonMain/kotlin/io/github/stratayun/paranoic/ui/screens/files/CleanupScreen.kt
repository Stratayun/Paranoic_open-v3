package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

/**
 * Cleanup of one device place (owner 26.11): exact duplicates and junk.
 * Everything picked goes to the trash, so it can be restored; nothing is erased here.
 */
@Composable
fun CleanupScreen(session: AppSession, space: Space, onClose: () -> Unit) {
    val scanner = session.cleanup
    LaunchedEffect(space.id) { if (scanner.state?.space?.id != space.id) scanner.scan(space) }
    val state = scanner.state
    fun key(item: FileItem) = item.location.storageId + ":" + item.location.path
    // Duplicates: every copy except the oldest is pre-selected. Junk: groups are selected as a whole.
    var picked by remember(state?.phase, state?.duplicates) {
        mutableStateOf(state?.duplicates.orEmpty().flatMap { it.items.drop(1) }.map(::key).toSet())
    }
    var junkPicked by remember(state?.phase) { mutableStateOf(setOf<JunkKind>()) }
    var confirm by remember { mutableStateOf(false) }
    val scheme = MaterialTheme.colorScheme
    // Large files and old downloads are never pre-selected: the owner picks each one.
    var reviewPicked by remember(state?.phase) { mutableStateOf(setOf<String>()) }
    val selectedItems = buildList {
        state?.duplicates.orEmpty().forEach { g -> g.items.filter { key(it) in picked }.forEach(::add) }
        state?.junk.orEmpty().filter { it.kind in junkPicked }.forEach { addAll(it.items) }
        (state?.large.orEmpty() + state?.oldDownloads.orEmpty()).filter { key(it) in reviewPicked }.forEach(::add)
    }.distinctBy(::key)
    val selectedBytes = selectedItems.fold(0uL) { sum, it -> sum + if (it.kind == EntryKind.File) it.bytes else 0uL }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = scheme.surface) {
            Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose, modifier = Modifier.actionDescription(tr("Назад", "Back"))) { AppIcon(AppSymbol.Back) }
                    BrandIcon(Res.drawable.ic_cleanup, size = 40.dp)
                    Spacer(Modifier.width(8.dp))
                    Column(Modifier.weight(1f)) {
                        Text(tr("Очистка", "Cleanup"), style = MaterialTheme.typography.headlineSmall)
                        Text(displayName(space.name), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                    }
                    TextButton(enabled = state?.running == false, onClick = { scanner.scan(space) }) { Text(tr("Заново", "Rescan")) }
                }
                state?.fault?.let { FaultNotice(it) }
                if (state?.running != false) {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(if (state?.phase == "hash") tr("Сравниваем содержимое: ${state.hashed} из ${state.toHash}", "Comparing content: ${state.hashed} of ${state.toHash}")
                        else tr("Просматриваем папки: ${state?.foldersScanned ?: 0}", "Scanning folders: ${state?.foldersScanned ?: 0}"),
                        color = scheme.onSurfaceVariant)
                }
                if (state != null && !state.running) {
                    if (state.truncated || state.unreadable > 0) Text(tr("Просмотрено не всё (ограничения или недоступные папки).", "Not everything was scanned (limits or unreadable folders)."),
                        style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                    if (state.duplicates.isEmpty() && state.junk.isEmpty() && state.large.isEmpty() && state.oldDownloads.isEmpty()) Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                        Text(tr("Ничего лишнего не найдено", "Nothing to clean up"), style = MaterialTheme.typography.titleMedium)
                    } else LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        if (state.junk.isNotEmpty()) item { SectionLabel(tr("Ненужное", "Junk")) }
                        items(state.junk, key = { it.kind.name }) { group ->
                            val on = group.kind in junkPicked
                            GroupedRow(group.kind.label(), GroupPosition.Single,
                                subtitle = tr("${group.items.size} шт.", "${group.items.size} items") + if (group.bytes > 0uL) " · " + humanSize(group.bytes) else "",
                                leading = { Checkbox(on, onCheckedChange = { junkPicked = if (on) junkPicked - group.kind else junkPicked + group.kind }) },
                                onClick = { junkPicked = if (on) junkPicked - group.kind else junkPicked + group.kind })
                        }
                        if (state.large.isNotEmpty()) item {
                            SectionLabel(tr("Крупные файлы (от 100 МБ): ${state.large.size}", "Large files (100 MB+): ${state.large.size}"))
                        }
                        items(state.large, key = { "large:" + key(it) }) { item ->
                            ReviewRow(item, key(item) in reviewPicked) { reviewPicked = if (key(item) in reviewPicked) reviewPicked - key(item) else reviewPicked + key(item) }
                        }
                        if (state.oldDownloads.isNotEmpty()) item {
                            SectionLabel(tr("Старые загрузки (больше полугода): ${state.oldDownloads.size}", "Old downloads (over six months): ${state.oldDownloads.size}"))
                        }
                        items(state.oldDownloads, key = { "old:" + key(it) }) { item ->
                            ReviewRow(item, key(item) in reviewPicked) { reviewPicked = if (key(item) in reviewPicked) reviewPicked - key(item) else reviewPicked + key(item) }
                        }
                        if (state.duplicates.isNotEmpty()) item {
                            SectionLabel(tr("Дубли: ${state.duplicates.size} групп, можно освободить ", "Duplicates: ${state.duplicates.size} groups, can free ") +
                                humanSize(state.duplicates.fold(0uL) { s, g -> s + g.wasted }))
                        }
                        items(state.duplicates, key = { it.hash }) { group ->
                            Surface(shape = MaterialTheme.shapes.large, color = scheme.surfaceContainerHigh) {
                                Column(Modifier.fillMaxWidth().padding(8.dp)) {
                                    Text(displayName(group.items.first().name) + " · " + humanSize(group.bytes) + tr(" · копий: ${group.items.size}", " · copies: ${group.items.size}"),
                                        style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    group.items.forEachIndexed { index, item ->
                                        val on = key(item) in picked
                                        Row(verticalAlignment = Alignment.CenterVertically) {
                                            Checkbox(on, onCheckedChange = { picked = if (on) picked - key(item) else picked + key(item) })
                                            Column(Modifier.weight(1f)) {
                                                Text("/" + displayName(item.location.path), style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                                if (index == 0) Text(tr("самый старый — оставляем", "oldest — kept"), style = MaterialTheme.typography.labelSmall, color = scheme.primary)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Button(enabled = selectedItems.isNotEmpty() && session.trashAvailable(selectedItems) && !session.writing,
                        onClick = { confirm = true }, modifier = Modifier.fillMaxWidth()) {
                        Text(tr("В корзину: ${selectedItems.size}", "To trash: ${selectedItems.size}") + if (selectedBytes > 0uL) " · " + humanSize(selectedBytes) else "")
                    }
                    if (selectedItems.isNotEmpty() && !session.trashAvailable(selectedItems))
                        Text(tr("Для этого места корзина недоступна — очистка не выполняется.", "No trash for this place — cleanup is not available."),
                            style = MaterialTheme.typography.bodySmall, color = scheme.error)
                }
            }
        }
    }
    if (confirm) AlertDialog(onDismissRequest = { confirm = false },
        title = { Text(tr("Переместить в корзину: ${selectedItems.size}?", "Move ${selectedItems.size} to trash?")) },
        text = { Text(tr("Файлы можно вернуть из корзины, пока она не очищена.", "Items can be restored from the trash until it is emptied.")) },
        confirmButton = { TextButton(onClick = { confirm = false; session.moveToTrash(selectedItems) { scanner.scan(space) } }) { Text(tr("В корзину", "Move to trash")) } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
private fun ReviewRow(item: FileItem, on: Boolean, toggle: () -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Checkbox(on, onCheckedChange = { toggle() })
        FileTypeIcon(item.name, folder = false, size = 36.dp)
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
            Text(displayName(item.name), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOfNotNull(humanSize(item.bytes), fileModifiedLabel(item.modifiedMs)).joinToString(" · ") +
                "  /" + displayName(item.location.path.substringBeforeLast('/', "")),
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun JunkKind.label(): String = when (this) {
    JunkKind.EmptyFiles -> tr("Пустые файлы", "Empty files")
    JunkKind.EmptyFolders -> tr("Пустые папки", "Empty folders")
    JunkKind.Temporary -> tr("Временные и недокачанные файлы", "Temporary and partial downloads")
    JunkKind.Installers -> tr("Установщики APK в Загрузках", "APK installers in Downloads")
    JunkKind.Thumbnails -> tr("Кэш миниатюр (.thumbnails)", "Thumbnail cache (.thumbnails)")
}
