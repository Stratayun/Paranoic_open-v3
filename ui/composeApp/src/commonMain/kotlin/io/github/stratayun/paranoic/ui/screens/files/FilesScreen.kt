package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.ui.draw.clip
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
        val canCreate = state.loaded && state.writable && !state.loading && !session.writing
        val canCreateText = canCreate && session.fileBatch == null && session.directWrite?.phase != DirectWritePhase.Unknown
        val canSelectAll = state.loaded && !state.loading && !session.writing && visible.isNotEmpty() &&
            (state.selected + visible.map { it.location }).size <= MAX_FILE_SELECTION
        var addMenu by remember(state.folder) { mutableStateOf(false) }
        var moreMenu by remember(state.folder, state.selected) { mutableStateOf(false) }
        @Composable fun Action(symbol: AppSymbol, label: String, enabled: Boolean, onClick: () -> Unit) =
            IconButton(onClick = onClick, enabled = enabled, modifier = Modifier.actionDescription(label)) { AppIcon(symbol) }
        if (state.selected.isEmpty()) Row(verticalAlignment = Alignment.CenterVertically) {
            Spacer(Modifier.weight(1f))
            Action(AppSymbol.Refresh, tr("Обновить", "Refresh"), session.core.connected && !state.loading) { inTab { session.refresh() } }
            Action(AppSymbol.SelectAll, tr("Выбрать все видимые (${visible.size})", "Select all visible (${visible.size})"), canSelectAll) {
                inTab { session.selectVisible(visible.map { it.location }) }
            }
            Box {
                FilledTonalIconButton(onClick = { addMenu = true }, enabled = canCreate,
                    modifier = Modifier.actionDescription(tr("Создать", "Create"))) { AppIcon(AppSymbol.Add) }
                DropdownMenu(addMenu, { addMenu = false }) {
                    DropdownMenuItem(text = { Text(tr("Новая папка", "New folder")) }, leadingIcon = { AppIcon(AppSymbol.NewFolder) },
                        onClick = { addMenu = false; inTab { creating = true } })
                    DropdownMenuItem(text = { Text(tr("Новый текстовый файл", "New text file")) }, leadingIcon = { AppIcon(AppSymbol.NewFile) },
                        enabled = canCreateText, onClick = { addMenu = false; inTab { creatingText = true } })
                }
            }
        } else Surface(shape = MaterialTheme.shapes.extraLarge, color = MaterialTheme.colorScheme.secondaryContainer) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Action(AppSymbol.Close, tr("Снять выбор", "Clear selection"), true) { inTab { session.clearSelection() } }
                Text(tr("Выбрано: ${state.selected.size}", "Selected: ${state.selected.size}"), Modifier.weight(1f),
                    style = MaterialTheme.typography.titleMedium)
                Action(AppSymbol.Copy, tr("Копировать в…", "Copy to…"), canAct && selectedItems.all { it.canCopy(state.spaces) }) {
                    if (selectedItems.size > 1) batchAction = FileBatchKind.Copy to selectedItems.toList()
                    else selected?.let { transferring = it to TransferMode.Copy }
                }
                Action(AppSymbol.MoveTo, tr("Перенести в…", "Move to…"), canAct && selectedItems.all { it.canMove(state.spaces) }) {
                    if (selectedItems.size > 1) batchAction = FileBatchKind.Move to selectedItems.toList()
                    else selected?.let { transferring = it to TransferMode.Move }
                }
                // SAF folder deletion is deliberately absent: deleteDocument can remove new children in a race.
                val deletable = selectedItems.none { it.kind == EntryKind.Directory && state.spaces.spaceOf(it.location)?.androidTree == true }
                if (deletable) Action(AppSymbol.Trash, tr("Удалить…", "Delete…"), canAct && selectedItems.all { it.canDelete(state.spaces) }) {
                    if (selectedItems.size > 1) batchAction = FileBatchKind.Delete to selectedItems.toList() else deleting = selected
                }
                Box {
                    Action(AppSymbol.More, tr("Ещё действия", "More actions"), true) { moreMenu = true }
                    DropdownMenu(moreMenu, { moreMenu = false }) {
                        DropdownMenuItem(text = { Text(tr("Переименовать…", "Rename…")) }, leadingIcon = { AppIcon(AppSymbol.Edit) },
                            enabled = actionable && selected?.canRename(state.spaces) == true, onClick = { moreMenu = false; renaming = selected })
                        DropdownMenuItem(text = { Text(tr("Сжать…", "Compress…")) }, leadingIcon = { AppIcon(AppSymbol.Archive) },
                            enabled = canAct && selectedItems.all { it.canCopy(state.spaces) },
                            onClick = { moreMenu = false; inTab { archiveSources = selectedItems; archiveAction = ArchiveAction.Create } })
                        if (selected?.kind == EntryKind.File) {
                            DropdownMenuItem(text = { Text(tr("Открыть архив…", "Open archive…")) }, leadingIcon = { AppIcon(AppSymbol.Archive) },
                                enabled = actionable, onClick = { moreMenu = false; inTab { archiveSources = listOf(selected); archiveAction = ArchiveAction.Open } })
                            val cryptoEnabled = actionable && session.directWrite?.phase != DirectWritePhase.Unknown
                            DropdownMenuItem(text = { Text(tr("Зашифровать файл…", "Encrypt file…")) }, leadingIcon = { AppIcon(AppSymbol.Vault) },
                                enabled = cryptoEnabled, onClick = { moreMenu = false; decrypting = false; cryptoSource = selected })
                            DropdownMenuItem(text = { Text(tr("Расшифровать age…", "Decrypt age…")) }, leadingIcon = { AppIcon(AppSymbol.Passwords) },
                                enabled = cryptoEnabled, onClick = { moreMenu = false; decrypting = true; cryptoSource = selected })
                        }
                        DropdownMenuItem(text = { Text(tr("Выбрать все видимые (${visible.size})", "Select all visible (${visible.size})")) },
                            leadingIcon = { AppIcon(AppSymbol.SelectAll) }, enabled = canSelectAll,
                            onClick = { moreMenu = false; inTab { session.selectVisible(visible.map { it.location }) } })
                    }
                }
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
            Text(if (state.spaces.isEmpty()) tr("Откройте меню ☰ и выберите место: память устройства или подключённую папку.", "Open the ☰ menu and pick a place: device storage or a connected folder.")
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
            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
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
    val scheme = MaterialTheme.colorScheme
    val symbol = fileSymbol(item)
    Surface(modifier = Modifier.fillMaxWidth().combinedClickable(onClickLabel = if (selectionActive) selectionLabel else openLabel,
        onLongClickLabel = selectionLabel, onLongClick = onSelect,
        onClick = { if (selectionActive || item.kind !in listOf(EntryKind.File, EntryKind.Directory)) onSelect() else onOpen() }),
        shape = MaterialTheme.shapes.large, color = if (selected) scheme.secondaryContainer else androidx.compose.ui.graphics.Color.Transparent) {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            if (selectionActive) Checkbox(checked = selected, onCheckedChange = { onSelect() },
                modifier = Modifier.semantics { contentDescription = selectionLabel })
            if (drag != null) FileDragHandle(item, drag)
            val preview = thumbnail
            if (preview != null) pngImage(preview, Modifier.size(48.dp).clip(MaterialTheme.shapes.medium).clearAndSetSemantics { })
            else ShapedIcon(symbol, MaterialTheme.shapes.medium,
                if (item.kind == EntryKind.Directory) scheme.primaryContainer else scheme.surfaceContainerHighest,
                if (item.kind == EntryKind.Directory) scheme.onPrimaryContainer else scheme.onSurfaceVariant)
            Column(Modifier.weight(1f)) {
                Text(displayName(item.name), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val modified = fileModifiedLabel(item.modifiedMs)
                val size = when (item.kind) {
                    EntryKind.Directory -> null
                    EntryKind.File -> if (item.sizeKnown) humanSize(item.bytes) else tr("размер неизвестен", "size unknown")
                    EntryKind.Symlink -> tr("ссылка", "symbolic link")
                    EntryKind.Other -> tr("другой тип", "other type")
                }
                val line = listOfNotNull(modified, size).joinToString(" · ")
                if (line.isNotEmpty()) Text(line, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                if (detailed) Text(displayName("/" + item.location.path), maxLines = 3, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
        }
    }
}

private val imageExt = setOf("png", "jpg", "jpeg", "gif", "bmp", "webp", "heic", "avif", "svg")
private val videoExt = setOf("mp4", "mkv", "webm", "avi", "mov", "3gp")
private val audioExt = setOf("mp3", "flac", "ogg", "opus", "m4a", "wav", "aac")
private val archiveExt = setOf("zip", "7z", "rar", "tar", "gz", "xz", "zst", "bz2")
private val lockedExt = setOf("age", "aes", "c9r", "pnc")

internal fun fileSymbol(item: FileItem): AppSymbol {
    if (item.kind == EntryKind.Directory) return AppSymbol.Folder
    val ext = item.name.substringAfterLast('.', "").lowercase()
    return when (ext) {
        in imageExt -> AppSymbol.Photo
        in videoExt -> AppSymbol.Video
        in audioExt -> AppSymbol.Audio
        in archiveExt -> AppSymbol.Archive
        in lockedExt -> AppSymbol.Vault
        else -> AppSymbol.Document
    }
}

/** 1536 -> "1,5 КБ"; binary units, one decimal below 10. */
@Composable
internal fun humanSize(bytes: ULong): String {
    val units = listOf(tr("Б", "B"), tr("КБ", "KB"), tr("МБ", "MB"), tr("ГБ", "GB"), tr("ТБ", "TB"))
    var value = bytes.toDouble(); var unit = 0
    while (value >= 1024 && unit < units.lastIndex) { value /= 1024; unit++ }
    val sep = tr(",", ".")
    val text = if (unit == 0 || value >= 10) value.toLong().toString()
        else { val tenths = (value * 10).toLong(); "${tenths / 10}$sep${tenths % 10}" }
    return "$text ${units[unit]}"
}
