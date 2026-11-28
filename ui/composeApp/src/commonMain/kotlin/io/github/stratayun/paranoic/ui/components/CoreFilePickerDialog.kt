package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.clickable
import io.github.stratayun.paranoic.resources.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
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
    var showHidden by remember { mutableStateOf(false) }
    val places = spaces.userPlaces().filter { !writable || it.writable }
    val scheme = MaterialTheme.colorScheme
    val canConfirm = core.connected && if (kind == CorePickKind.Directory) ready else eligible && !loading
    fun confirm() {
        if (kind == CorePickKind.Directory) {
            val node = trail.lastOrNull()
            if (ready && node != null) onPick(listOf(FileItem(node.location, node.name, EntryKind.Directory, 0uL)))
        } else if (eligible && !loading) onPick(selected.toList())
    }
    // Full-screen picker (reference: Files app): location bar, rows with icons, checkmarks, a confirm bar.
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().trackSecretActivity(), color = scheme.surface) {
            Column(Modifier.fillMaxSize().safeDrawingPadding()) {
                Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onDismiss, modifier = Modifier.actionDescription(tr("Отмена", "Cancel"))) { AppIcon(AppSymbol.Close) }
                    Column(Modifier.weight(1f)) {
                        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1)
                        Text(if (kind == CorePickKind.Directory) tr("Выберите папку", "Choose a folder")
                            else tr("Отмечено: ${selected.size} из $maxSelection", "Selected: ${selected.size} of $maxSelection"),
                            style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                    }
                    if (trail.isNotEmpty()) IconButton(onClick = { showHidden = !showHidden },
                        modifier = Modifier.actionDescription(if (showHidden) tr("Скрыть служебные папки", "Hide dot folders") else tr("Показать служебные папки", "Show dot folders"))) {
                        AppIcon(if (showHidden) AppSymbol.Info else AppSymbol.Search)
                    }
                }
                Surface(Modifier.fillMaxWidth().padding(horizontal = 12.dp), shape = MaterialTheme.shapes.extraLarge, color = scheme.surfaceContainer) {
                    Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(enabled = trail.isNotEmpty() && !loading, onClick = { trail = trail.dropLast(1) },
                            modifier = Modifier.actionDescription(tr("Выше", "Up"))) { AppIcon(AppSymbol.Up) }
                        Text(trail.joinToString(" › ") { displayName(it.name) }.ifEmpty { tr("Места", "Places") },
                            Modifier.weight(1f), style = MaterialTheme.typography.titleSmall, maxLines = 1)
                        IconButton(enabled = folder != null && !loading, onClick = { attempt++ },
                            modifier = Modifier.actionDescription(tr("Обновить", "Refresh"))) { AppIcon(AppSymbol.Refresh) }
                    }
                }
                if (loading) LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp))
                fault?.let { FaultNotice(it, Modifier.padding(horizontal = 12.dp)) }
                LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    if (trail.isEmpty()) {
                        items(places, key = { it.id }) { space ->
                            PickerRow(displayName(space.name), if (space.cloud) AppSymbol.Cloud else AppSymbol.Phone, folder = true,
                                checked = null, onClick = { trail = listOf(FolderNode(space.root, space.name)) })
                        }
                        if (places.isEmpty()) item { Text(tr("Нет подходящих мест.", "No suitable places."), Modifier.padding(16.dp)) }
                    } else {
                        val rows = current?.items.orEmpty()
                            .filter { it.kind == EntryKind.Directory || kind != CorePickKind.Directory && it.kind == EntryKind.File }
                            .filter { showHidden || !it.name.startsWith(".") }
                            .sortedWith(compareBy<FileItem>({ it.kind != EntryKind.Directory }, { it.name.lowercase() }))
                        items(rows, key = { it.location.storageId + ":" + it.location.path }) { item ->
                            val pickable = kind != CorePickKind.Directory && (item.kind == EntryKind.File || kind == CorePickKind.FilesAndFolders)
                            val checked = selected.any { it.location == item.location }
                            fun toggle() {
                                selected = if (!checked) { if (maxSelection == 1) listOf(item) else if (selected.size < maxSelection) selected + item else selected }
                                    else selected.filterNot { it.location == item.location }
                            }
                            PickerRow(displayName(item.name), if (item.kind == EntryKind.Directory) AppSymbol.Folder else pickerSymbol(item.name),
                                folder = item.kind == EntryKind.Directory, checked = if (pickable) checked else null,
                                onCheck = { toggle() },
                                onClick = { if (item.kind == EntryKind.Directory) { if (trail.size < 64) trail = trail + FolderNode(item.location, item.name) } else if (pickable) toggle() })
                        }
                        if (current != null && rows.isEmpty()) item {
                            Text(tr("Здесь пусто.", "Nothing here."), Modifier.padding(16.dp), color = scheme.onSurfaceVariant)
                        }
                    }
                }
                Surface(color = scheme.surfaceContainer) {
                    Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (kind != CorePickKind.Directory) TextButton(enabled = selected.isNotEmpty(), onClick = { selected = emptyList() }) { Text(tr("Сбросить", "Reset")) }
                        Spacer(Modifier.weight(1f))
                        Button(enabled = canConfirm, onClick = ::confirm, modifier = Modifier.height(48.dp)) {
                            Text(if (kind == CorePickKind.Directory) tr("Выбрать эту папку", "Choose this folder")
                                else tr("Готово (${selected.size})", "Done (${selected.size})"))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PickerRow(title: String, symbol: AppSymbol, folder: Boolean, checked: Boolean?, onCheck: () -> Unit = {}, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.large, color = if (checked == true) scheme.secondaryContainer else Color.Transparent) {
        Row(Modifier.clickable(onClick = onClick).heightIn(min = 56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            if (symbol == AppSymbol.Phone || symbol == AppSymbol.Cloud) BrandIcon(if (symbol == AppSymbol.Cloud) Res.drawable.ic_cloud
                else Res.drawable.ic_internal_storage, size = 44.dp)
            else FileTypeIcon(title, folder, size = 44.dp)
            Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, maxLines = 2)
            if (checked != null) Checkbox(checked = checked, onCheckedChange = { onCheck() })
            else if (folder) AppIcon(AppSymbol.Forward)
        }
    }
}

private fun pickerSymbol(name: String): AppSymbol = when (name.substringAfterLast('.', "").lowercase()) {
    "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp" -> AppSymbol.Photo
    "mp4", "mkv", "webm", "mov", "avi", "3gp" -> AppSymbol.Video
    "mp3", "flac", "ogg", "opus", "m4a", "wav", "aac" -> AppSymbol.Audio
    "zip", "7z", "rar", "tar", "gz" -> AppSymbol.Archive
    "age" -> AppSymbol.Vault
    else -> AppSymbol.Document
}
