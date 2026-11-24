package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.screens.documents.ArchiveViewer
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.theme.LocalThemeTokens

@OptIn(ExperimentalLayoutApi::class, ExperimentalMaterial3Api::class)
@Composable
fun FilesScreen(session: AppSession, view: FileView, onView: (FileView) -> Unit, onOpenFile: (FileItem) -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit, tabId: String = session.fileWorkspace.active,
    drag: FileDragHandler? = null, onOpenPlaces: (() -> Unit)? = null) {
    val tab = session.fileWorkspace.tab(tabId) ?: return
    val state = tab.files
    val options = tab.options
    fun inTab(action: () -> Unit) { if (session.activateFileTab(tabId)) action() }
    var creating by remember(state.folder) { mutableStateOf(false) }
    var creatingText by remember(state.folder, session.secretsEpoch) { mutableStateOf(false) }
    var transferring by remember(state.folder) { mutableStateOf<Pair<FileItem, TransferMode>?>(null) }
    var renaming by remember(state.folder) { mutableStateOf<FileItem?>(null) }
    var deleting by remember(state.folder) { mutableStateOf<FileItem?>(null) }
    var batchAction by remember(state.folder, session.secretsEpoch) { mutableStateOf<Pair<FileBatchKind, List<FileItem>>?>(null) }
    var cryptoMenu by remember(state.folder, state.selected, session.secretsEpoch) { mutableStateOf(false) }
    var cryptoSource by remember(state.folder, session.secretsEpoch) { mutableStateOf<FileItem?>(null) }
    var decrypting by remember { mutableStateOf(false) }
    var archiveSources by remember(state.folder, session.secretsEpoch) { mutableStateOf<List<FileItem>>(emptyList()) }
    var archiveAction by remember(state.folder, session.secretsEpoch) { mutableStateOf<ArchiveAction?>(null) }
    var showingArchive by remember(session.secretsEpoch) { mutableStateOf(false) }
    val visible = remember(state.entries, options) { visibleFiles(state.entries, options) }
    val selected = state.entries.singleOrNull { it.location in state.selected }
    val selectedItems = state.entries.filter { it.location in state.selected }
    val canAct = state.loaded && !state.loading && !session.writing && session.core.connected && session.fileBatch == null &&
        selectedItems.isNotEmpty() && selectedItems.size == state.selected.size
    val actionable = canAct && state.selected.size == 1
    val padding = LocalThemeTokens.current.pagePadding
    if (showingArchive && session.archives.state.source != null) { ArchiveViewer(session) { showingArchive = false }; return }
    PullToRefreshBox(isRefreshing = state.loading, onRefresh = {
        if (session.core.connected && !session.writing && !state.loading) inTab { session.refresh() }
    }, modifier = Modifier.fillMaxSize()) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
    val controlsHeight = maxHeight * 0.50f
    Column(Modifier.fillMaxSize().padding(horizontal = padding), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(Modifier.fillMaxWidth().heightIn(max = controlsHeight).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (session.fileWorkspace.active == tabId) FileBatchCard(session)
        FilesNavigation(state.trail) { index -> inTab { session.breadcrumb(index) } }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(onClick = { inTab { session.refresh() } }, enabled = session.core.connected && !state.loading) {
                Text(tr("Обновить", "Refresh"))
            }
            TextButton(onClick = { inTab { creating = true } },
                enabled = state.loaded && state.writable && !state.loading && !session.writing) {
                Text(tr("Новая папка", "New folder"))
            }
            TextButton(onClick = { inTab { creatingText = true } }, enabled = state.loaded && state.writable && !state.loading &&
                !session.writing && session.fileBatch == null && session.directWrite?.phase != DirectWritePhase.Unknown) {
                Text(tr("Новый текстовый файл", "New text file"))
            }
            TextButton(onClick = { inTab { session.selectVisible(visible.map { it.location }) } },
                enabled = state.loaded && !state.loading && !session.writing && visible.isNotEmpty() &&
                    (state.selected + visible.map { it.location }).size <= MAX_FILE_SELECTION) {
                Text(tr("Выбрать все видимые (${visible.size})", "Select all visible (${visible.size})"))
            }
            if (state.selected.isNotEmpty()) {
                TextButton(enabled = canAct &&
                    selectedItems.all { it.canCopy(state.spaces) }, onClick = { inTab { archiveSources = selectedItems; archiveAction = ArchiveAction.Create } }) {
                    Text(tr("Сжать…", "Compress…"))
                }
                if (selected?.kind == EntryKind.File) TextButton(enabled = actionable, onClick = {
                    inTab { archiveSources = listOf(selected); archiveAction = ArchiveAction.Open }
                }) { Text(tr("Открыть архив…", "Open archive…")) }
                if (selected?.kind == EntryKind.File) Box {
                    TextButton(onClick = { inTab { cryptoMenu = true } }, enabled = actionable && session.directWrite?.phase != DirectWritePhase.Unknown) {
                        Text(tr("Шифрование…", "Encryption…"))
                    }
                    DropdownMenu(expanded = cryptoMenu, onDismissRequest = { cryptoMenu = false }) {
                        DropdownMenuItem(text = { Text(tr("Зашифровать файл…", "Encrypt file…")) }, onClick = {
                            cryptoMenu = false; decrypting = false; cryptoSource = selected
                        })
                        DropdownMenuItem(text = { Text(tr("Расшифровать age…", "Decrypt age…")) }, onClick = {
                            cryptoMenu = false; decrypting = true; cryptoSource = selected
                        })
                    }
                }
                TextButton(onClick = {
                    if (selectedItems.size > 1) batchAction = FileBatchKind.Copy to selectedItems.toList()
                    else selected?.let { transferring = it to TransferMode.Copy }
                }, enabled = canAct && selectedItems.all { it.canCopy(state.spaces) }) {
                    Text(tr("Копировать в…", "Copy to…"))
                }
                TextButton(onClick = {
                    if (selectedItems.size > 1) batchAction = FileBatchKind.Move to selectedItems.toList()
                    else selected?.let { transferring = it to TransferMode.Move }
                }, enabled = canAct && selectedItems.all { it.canMove(state.spaces) }) {
                    Text(tr("Перенести в…", "Move to…"))
                }
                TextButton(onClick = { renaming = selected },
                    enabled = actionable && selected?.canRename(state.spaces) == true) {
                    Text(tr("Переименовать…", "Rename…"))
                }
                // SAF folder deletion is deliberately absent: deleteDocument can remove new children in a race.
                if (selectedItems.none { it.kind == EntryKind.Directory && state.spaces.spaceOf(it.location)?.androidTree == true }) {
                    TextButton(onClick = {
                        if (selectedItems.size > 1) batchAction = FileBatchKind.Delete to selectedItems.toList() else deleting = selected
                    }, enabled = canAct && selectedItems.all { it.canDelete(state.spaces) }) {
                        Text(tr("Удалить…", "Delete…"))
                    }
                }
                TextButton(onClick = { inTab { session.clearSelection() } }) { Text(tr("Снять выбор", "Clear selection")) }
                Text(tr("Выбрано: ${state.selected.size}", "Selected: ${state.selected.size}"), Modifier.padding(12.dp))
            }
        }
        if (state.selected.isNotEmpty()) {
            val visibleLocations = visible.map { it.location }.toSet()
            val outsideFilter = state.selected.count { it !in visibleLocations }
            if (outsideFilter > 0) Text(tr("Выбрано вне текущего фильтра: $outsideFilter. Эти объекты тоже войдут в операцию.",
                "Selected outside the current filter: $outsideFilter. These entries will also be included."))
        }
        if (visible.size > MAX_FILE_SELECTION) Text(tr("За один раз можно выбрать до $MAX_FILE_SELECTION объектов. Уточните фильтр или выберите вручную.",
            "Select up to $MAX_FILE_SELECTION entries at a time. Narrow the filter or select entries individually."))
        if (selected?.let { state.spaces.spaceOf(it.location)?.androidTree } == true) Text(tr(
            "Это подключение Android не поддерживает переименование и перенос внутри него. Удаление и перенос папок отключены.",
            "This Android connection does not support renaming or moves within it. Folder deletion and moving are disabled."))
        if (state.folder != null) FileListControls(options, view, visible.size, state.entries.size,
            onOptions = { session.fileOptions(tabId, it) }, onView = { next -> session.fileView(tabId, next); onView(next) })
        if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        state.fault?.let { FaultNotice(it) }
        if (state.hidden > 0) Text(tr("Скрыто объектов с недопустимыми или повторяющимися именами: ${state.hidden}",
            "Hidden entries with unsupported or duplicate names: ${state.hidden}"))
        if (state.folder == null && session.core.connected) {
            Text(if (state.spaces.isEmpty()) tr("Сначала добавьте папку через «Места». Доступ ко всем файлам не требуется.", "Add a folder through Places first. All-files access is not required.")
                else tr("Откройте подключённую папку через «Места».", "Open a connected folder through Places."))
            if (onOpenPlaces != null) TextButton(onClick = { inTab { onOpenPlaces() } }) { Text(tr("Открыть «Места»", "Open Places")) }
            else Text(tr("Список мест находится в боковой панели.", "Places are listed in the side panel."))
        }
        if (state.loaded && visible.isEmpty() && !state.loading) {
            Text(if (state.entries.isEmpty()) tr("Папка пустая. Создайте папку, текстовый файл или скопируйте сюда нужные данные.",
                "This folder is empty. Create a folder or text file, or copy files here.") else tr("Текущий фильтр скрыл все загруженные объекты.", "The current filter hides all loaded entries."))
            if (state.entries.isNotEmpty()) TextButton(onClick = { session.fileOptions(tabId, FileListOptions(showHidden = true)) }) {
                Text(tr("Сбросить фильтр и показать скрытые", "Reset filter and show hidden"))
            }
        }
        }
        if (view != FileView.Grid) {
            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(visible, key = { it.location.storageId + ":" + it.location.path }) { item ->
                    FileRow(session.core, pngImage, item, item.location in state.selected, state.selected.isNotEmpty(), { inTab { session.select(item.location) } }, {
                        inTab { if (item.kind == EntryKind.Directory) session.openFolder(item) else onOpenFile(item) }
                    }, drag = drag?.takeIf { item.canCopy(state.spaces) }, detailed = view == FileView.Detailed)
                }
            }
        } else {
            LazyVerticalGrid(columns = GridCells.Adaptive(if (drag == null) 180.dp else 260.dp), modifier = Modifier.weight(1f),
                horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(visible, key = { it.location.storageId + ":" + it.location.path }) { item ->
                    FileRow(session.core, pngImage, item, item.location in state.selected, state.selected.isNotEmpty(), { inTab { session.select(item.location) } }, {
                        inTab { if (item.kind == EntryKind.Directory) session.openFolder(item) else onOpenFile(item) }
                    }, drag = drag?.takeIf { item.canCopy(state.spaces) })
                }
            }
        }
    }
    }
    }
    if (creating) {
        val parent = state.folder
        if (parent != null) CreateDirectoryDialog(onDismiss = { creating = false }) { name ->
            creating = false
            inTab { session.createDirectory(parent, name) }
        }
    }
    if (creatingText) state.folder?.let { parent ->
        CreateTextDialog(state.entries.map { it.name }.toSet(), onDismiss = { creatingText = false }) { name ->
            creatingText = false
            inTab { session.createText(parent, name) }
        }
    }
    transferring?.let { (source, mode) ->
        TransferDestinationDialog(session.core, source, state.spaces, mode, onDismiss = { transferring = null },
            onTransfer = { parent, name, conflict, replacement ->
                transferring = null
                inTab { session.transfer(source, parent, name, mode, conflict, replacement) }
            })
    }
    renaming?.let { source ->
        RenameEntryDialog(source, onDismiss = { renaming = null }) { name ->
            renaming = null
            inTab { session.rename(source, name) }
        }
    }
    deleting?.let { source ->
        DeleteEntryDialog(source, onDismiss = { deleting = null }) { recursive ->
            deleting = null
            inTab { session.delete(source, recursive) }
        }
    }
    batchAction?.let { (kind, sources) ->
        FileBatchDialog(session.core, state.spaces, sources, kind, onDismiss = { batchAction = null }) { plan ->
            batchAction = null
            inTab { session.submitFileBatch(plan) }
        }
    }
    cryptoSource?.let { source -> FileCryptoDialog(session, source, decrypting) { cryptoSource = null } }
    archiveAction?.let { action -> ArchiveDialog(session, archiveSources, action, onDismiss = { archiveAction = null },
        onOpened = { showingArchive = true }) }
}

@Composable
@OptIn(ExperimentalFoundationApi::class)
private fun FileRow(core: CoreGateway, pngImage: @Composable (ByteArray, Modifier) -> Unit,
    item: FileItem, selected: Boolean, selectionActive: Boolean, onSelect: () -> Unit, onOpen: () -> Unit,
    drag: FileDragHandler? = null, detailed: Boolean = false) {
    val selectionLabel = tr("Выбрать ", "Select ") + displayName(item.name)
    val openLabel = tr("Открыть ", "Open ") + displayName(item.name)
    // Same path and size but a new modification time = a different picture.
    var thumbnail by remember(item.location, item.bytes, item.modifiedMs) { mutableStateOf<ByteArray?>(null) }
    LaunchedEffect(core, item.location, item.bytes, item.modifiedMs) {
        if (item.kind == EntryKind.File && item.name.substringAfterLast('.', "").lowercase() in
            listOf("png", "jpg", "jpeg", "gif", "bmp", "webp")) {
            // Extension only schedules work; Rust validates actual image magic.
            when (val result = core.imagePng(item.location, 64)) {
                is CoreResult.Value -> thumbnail = result.value
                is CoreResult.Failure -> Unit
            }
        }
    }
    DisposableEffect(item.location, item.bytes, item.modifiedMs) { onDispose { thumbnail?.fill(0); thumbnail = null } }
    Surface(modifier = Modifier.combinedClickable(onClickLabel = if (selectionActive) selectionLabel else openLabel,
        onLongClickLabel = selectionLabel, onLongClick = onSelect,
        onClick = { if (selectionActive || item.kind !in listOf(EntryKind.File, EntryKind.Directory)) onSelect() else onOpen() }),
        shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
        Row(Modifier.fillMaxWidth().padding(LocalThemeTokens.current.itemPadding),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Checkbox(checked = selected, onCheckedChange = { onSelect() },
                modifier = Modifier.semantics { contentDescription = selectionLabel })
            if (drag != null) FileDragHandle(item, drag)
            val preview = thumbnail
            if (preview != null) pngImage(preview, Modifier.size(48.dp).clearAndSetSemantics { })
            else AppIcon(if (item.kind == EntryKind.Directory) AppSymbol.Folder else AppSymbol.Document)
            Column(Modifier.weight(1f)) {
                Text(displayName(item.name), maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(when (item.kind) {
                    EntryKind.Directory -> tr("Папка", "Folder")
                    EntryKind.File -> if (item.sizeKnown) tr("${item.bytes} байт", "${item.bytes} bytes")
                        else tr("Размер неизвестен", "Size unknown")
                    EntryKind.Symlink -> tr("Ссылка", "Symbolic link")
                    EntryKind.Other -> tr("Другой тип", "Other type")
                }, style = MaterialTheme.typography.bodySmall)
                if (detailed) {
                    Text(displayName("/" + item.location.path), maxLines = 3, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                    fileModifiedLabel(item.modifiedMs)?.let { Text(tr("Изменён: ", "Modified: ") + it, style = MaterialTheme.typography.bodySmall) }
                }
                if (item.kind == EntryKind.Directory || item.kind == EntryKind.File)
                    TextButton(onClick = onOpen, modifier = Modifier.actionDescription(openLabel)) { Text(tr("Открыть", "Open")) }
            }
        }
    }
}
