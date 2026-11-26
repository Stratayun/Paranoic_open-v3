package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun FileCategory.label(): String = when (this) {
    FileCategory.Images -> tr("Изображения", "Pictures")
    FileCategory.Videos -> tr("Фильмы", "Movies")
    FileCategory.Music -> tr("Музыка", "Music")
    FileCategory.Documents -> tr("Документы", "Documents")
    FileCategory.All -> tr("Поиск", "Search")
    FileCategory.Archives -> tr("Архивы", "Archives")
}

/** All files of one kind in a place (drawer quick places). Tap opens; long press shows where it is. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CategoryScreen(session: AppSession, state: CategoryState, onOpenFile: (FileItem) -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit, clipboard: FileClipboard? = null,
    onSendToSafe: ((List<FileItem>) -> Unit)? = null) {
    val scheme = MaterialTheme.colorScheme
    var sheet by remember(state.category) { mutableStateOf<FileItem?>(null) }
    var info by remember(state.category) { mutableStateOf<FileItem?>(null) }
    var deleting by remember(state.category) { mutableStateOf<FileItem?>(null) }
    var permanent by remember { mutableStateOf(false) }
    var renaming by remember(state.category) { mutableStateOf<FileItem?>(null) }
    var transferring by remember(state.category) { mutableStateOf<Pair<FileItem, TransferMode>?>(null) }
    var rescan by remember(state.category) { mutableStateOf(false) }
    // Multi-select (owner 26.11): long press starts it, taps toggle, the bar acts on all selected.
    var selected by remember(state.category, state.space.id) { mutableStateOf(setOf<String>()) }
    var sort by remember { mutableStateOf(CategorySort.Newest) }
    var hideSystemSounds by remember { mutableStateOf(true) }
    var trashingMany by remember(state.category) { mutableStateOf<List<FileItem>?>(null) }
    // age for single files (owner 26.11: was only reachable from the folder view).
    var crypting by remember(state.category) { mutableStateOf<Pair<FileItem, Boolean>?>(null) }
    var batchCrypto by remember(state.category) { mutableStateOf<Pair<List<FileItem>, Boolean>?>(null) }
    fun key(item: FileItem) = item.location.storageId + ":" + item.location.path
    val shown = remember(state.items, sort, hideSystemSounds, state.category) {
        state.items.filter { state.category != FileCategory.Music || !hideSystemSounds || !isSystemSound(it) }.sortedWith(sort.comparator)
    }
    val selecting = selected.isNotEmpty()
    val selectedItems = shown.filter { key(it) in selected }
    fun toggle(item: FileItem) { val k = key(item); selected = if (k in selected) selected - k else selected + k }
    // After a write finishes, walk again so the list shows the result (moved, renamed or deleted files).
    LaunchedEffect(session.writing, rescan) {
        if (rescan && !session.writing) { rescan = false; session.categories.open(state.space, state.category, state.query) }
    }
    val canWrite = !session.writing && session.core.connected && session.fileBatch == null
    fun showInFolder(item: FileItem) {
        // openPath takes a path relative to the place's root; locations carry the full path.
        val parent = item.location.path.substringBeforeLast('/', "")
        val root = state.space.root.path
        val relative = if (root.isNotEmpty() && (parent == root || parent.startsWith("$root/"))) parent.removePrefix(root).trimStart('/') else parent
        session.categories.close()
        session.openPath(state.space, relative)
    }
    Column(Modifier.fillMaxSize().padding(horizontal = 12.dp)) {
        Surface(Modifier.fillMaxWidth().padding(vertical = 8.dp), shape = MaterialTheme.shapes.extraLarge, color = scheme.surfaceContainer) {
            if (selecting) Row(Modifier.padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { selected = emptySet() }, modifier = Modifier.actionDescription(tr("Снять выбор", "Clear selection"))) { AppIcon(AppSymbol.Close) }
                // One file: its name and the full action sheet (show in folder, properties, rename…).
                Text(selectedItems.singleOrNull()?.let { displayName(it.name) } ?: tr("Выбрано: ${selectedItems.size}", "Selected: ${selectedItems.size}"),
                    Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                IconButton(onClick = { selected = if (selectedItems.size == shown.size) emptySet() else shown.map(::key).toSet() },
                    modifier = Modifier.actionDescription(tr("Выбрать все", "Select all"))) { AppIcon(AppSymbol.SelectAll) }
                if (onSendToSafe != null) IconButton(enabled = canWrite && selectedItems.all { it.canMove(session.files.spaces) },
                    onClick = { onSendToSafe(selectedItems); selected = emptySet() },
                    modifier = Modifier.actionDescription(tr("В Сейф", "To the Safe"))) { BrandIcon(Res.drawable.ic_safe, size = 32.dp) }
                if (clipboard != null) IconButton(enabled = selectedItems.all { it.canMove(session.files.spaces) },
                    onClick = { clipboard.put(selectedItems, cut = true); selected = emptySet() },
                    modifier = Modifier.actionDescription(tr("Вырезать", "Cut"))) { AppIcon(AppSymbol.Cut) }
                if (selectedItems.size > 1 && selectedItems.all { it.kind == EntryKind.File }) {
                    val allAge = selectedItems.all { isEncrypted(it) }
                    IconButton(enabled = canWrite && session.directWrite?.phase != DirectWritePhase.Unknown,
                        onClick = { batchCrypto = selectedItems to allAge },
                        modifier = Modifier.actionDescription(if (allAge) tr("Расшифровать", "Decrypt") else tr("Зашифровать (age)", "Encrypt (age)"))) {
                        AppIcon(if (allAge) AppSymbol.Passwords else AppSymbol.Vault)
                    }
                }
                IconButton(enabled = canWrite && selectedItems.all { it.canDelete(session.files.spaces) },
                    onClick = { trashingMany = selectedItems },
                    modifier = Modifier.actionDescription(tr("Удалить", "Delete"))) { AppIcon(AppSymbol.Trash) }
                selectedItems.singleOrNull()?.let { one ->
                    IconButton(onClick = { selected = emptySet(); showInFolder(one) },
                        modifier = Modifier.actionDescription(tr("Показать в папке", "Show in folder"))) { AppIcon(AppSymbol.Folder) }
                    IconButton(onClick = { selected = emptySet(); sheet = one }, modifier = Modifier.actionDescription(tr("Действия", "Actions"))) { AppIcon(AppSymbol.More) }
                }
            } else Row(Modifier.padding(start = 4.dp, end = 12.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = session.categories::close, modifier = Modifier.actionDescription(tr("Закрыть", "Close"))) { AppIcon(AppSymbol.Back) }
                Column(Modifier.weight(1f)) {
                    Text(state.category.label(), style = MaterialTheme.typography.titleMedium)
                    Text(displayName(state.space.name) + " · " + tr("найдено ${shown.size}", "${shown.size} found") +
                        if (state.running) tr(" · ищем…", " · searching…") else if (state.truncated) tr(" · показаны первые", " · partial")
                            else if (state.incomplete) tr(" · неполный: ${state.unreadable} папок недоступно", " · incomplete: ${state.unreadable} folders unreadable") else "",
                        style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
            }
        }
        // Smart search: name part across the whole place, narrowed by kind; re-runs 350 ms after typing stops.
        var query by remember(state.space.id) { mutableStateOf(state.query) }
        LaunchedEffect(query) {
            if (query == state.query) return@LaunchedEffect
            kotlinx.coroutines.delay(350)
            session.categories.open(state.space, state.category, query)
        }
        OutlinedTextField(query, { if (it.length <= 128) query = it }, Modifier.fillMaxWidth(), singleLine = true,
            shape = MaterialTheme.shapes.extraLarge, leadingIcon = { AppIcon(AppSymbol.Search) },
            placeholder = { Text(tr("Искать по имени на всём устройстве", "Search names on the whole device")) },
            trailingIcon = { if (query.isNotEmpty()) IconButton(onClick = { query = "" }) { AppIcon(AppSymbol.Close) } })
        androidx.compose.foundation.lazy.LazyRow(Modifier.fillMaxWidth().padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            items(FileCategory.entries) { kind ->
                FilterChip(selected = kind == state.category, onClick = { session.categories.open(state.space, kind, query) },
                    label = { Text(kind.label()) }, leadingIcon = { AppIcon(fileSymbolFor(kind), Modifier.size(18.dp)) })
            }
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            var sortMenu by remember { mutableStateOf(false) }
            Box {
                AssistChip(onClick = { sortMenu = true }, label = { Text(sort.label()) }, leadingIcon = { AppIcon(AppSymbol.Sort, Modifier.size(18.dp)) })
                DropdownMenu(sortMenu, { sortMenu = false }) {
                    CategorySort.entries.forEach { option ->
                        DropdownMenuItem(text = { Text(option.label()) }, onClick = { sort = option; sortMenu = false },
                            trailingIcon = { if (option == sort) Text("✓") })
                    }
                }
            }
            if (state.category == FileCategory.Music) FilterChip(selected = hideSystemSounds, onClick = { hideSystemSounds = !hideSystemSounds },
                label = { Text(tr("Без системных звуков", "No system sounds")) })
        }
        if (state.running) LinearProgressIndicator(Modifier.fillMaxWidth())
        if (state.category == FileCategory.All && query.isBlank()) Text(tr("Введите часть имени файла.", "Type part of a file name."),
            Modifier.padding(16.dp), color = scheme.onSurfaceVariant)
        state.fault?.let { FaultNotice(it) }
        if (!state.running && shown.isEmpty() && state.fault == null && !(state.category == FileCategory.All && query.isBlank())) Column(Modifier.fillMaxWidth().padding(top = 48.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            ShapedIcon(fileSymbolFor(state.category), ExpressiveShapes.Sunny, scheme.surfaceContainerHighest, scheme.onSurfaceVariant, size = 96.dp)
            Text(tr("Ничего не найдено", "Nothing found"), style = MaterialTheme.typography.titleMedium)
        }
        if (state.category == FileCategory.Images) LazyVerticalGrid(GridCells.Adaptive(112.dp), Modifier.weight(1f),
            horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            items(shown, key = { key(it) }) { item ->
                val picked = key(item) in selected
                Box(Modifier.aspectRatio(1f).clip(MaterialTheme.shapes.medium)
                    .combinedClickable(onLongClick = { toggle(item) }, onClick = { if (selecting) toggle(item) else onOpenFile(item) })) {
                    Thumbnail(session.core, item, pngImage, Modifier.fillMaxSize())
                    if (selecting) Box(Modifier.fillMaxSize().then(if (picked) Modifier.background(scheme.primary.copy(alpha = 0.35f)) else Modifier)) {
                        Checkbox(checked = picked, onCheckedChange = { toggle(item) }, modifier = Modifier.align(Alignment.TopEnd))
                    }
                }
            }
        } else LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            items(shown, key = { key(it) }) { item ->
                val picked = key(item) in selected
                Row(Modifier.fillMaxWidth().clip(MaterialTheme.shapes.large).then(if (picked) Modifier.background(scheme.secondaryContainer) else Modifier)
                    .combinedClickable(onLongClick = { toggle(item) }, onClick = { if (selecting) toggle(item) else onOpenFile(item) })
                    .heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    FileTypeIcon(item.name, false)
                    Column(Modifier.weight(1f)) {
                        Text(displayName(item.name), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("/" + displayName(item.location.path.substringBeforeLast('/', "")) + " · " +
                            (if (item.sizeKnown) humanSize(item.bytes) else ""), style = MaterialTheme.typography.bodySmall,
                            color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    if (selecting) Checkbox(checked = picked, onCheckedChange = { toggle(item) })
                    else IconButton(onClick = { sheet = item }, modifier = Modifier.actionDescription(tr("Действия", "Actions"))) { AppIcon(AppSymbol.More) }
                }
            }
        }
    }
    sheet?.let { item ->
        FileActionSheet(item.name, "/" + displayName(item.location.path.substringBeforeLast('/', "")), fileSymbol(item), listOf(
            SheetGroup(tr("Открыть", "Open"), listOf(
                SheetAction(tr("Открыть", "Open"), AppSymbol.Forward, run = { sheet = null; onOpenFile(item) }),
                SheetAction(tr("Показать в папке", "Show in folder"), AppSymbol.Folder, run = { sheet = null; showInFolder(item) }),
                SheetAction(tr("Свойства", "Properties"), AppSymbol.Info, run = { sheet = null; info = item }),
            )),
            SheetGroup(tr("Упорядочить", "Organize"), listOfNotNull(
                if (clipboard != null) SheetAction(tr("Копировать", "Copy"), AppSymbol.Copy, item.canCopy(session.files.spaces),
                    run = { sheet = null; clipboard.put(listOf(item), cut = false) }) else null,
                if (clipboard != null) SheetAction(tr("Вырезать", "Cut"), AppSymbol.Cut, item.canMove(session.files.spaces),
                    run = { sheet = null; clipboard.put(listOf(item), cut = true) }) else null,
                SheetAction(tr("Копировать в…", "Copy to…"), AppSymbol.Copy, canWrite && item.canCopy(session.files.spaces),
                    run = { sheet = null; transferring = item to TransferMode.Copy }),
                SheetAction(tr("Переместить в…", "Move to…"), AppSymbol.MoveTo, canWrite && item.canMove(session.files.spaces),
                    run = { sheet = null; transferring = item to TransferMode.Move }),
                SheetAction(tr("Переименовать", "Rename"), AppSymbol.Edit, canWrite && item.canRename(session.files.spaces),
                    run = { sheet = null; renaming = item }),
                SheetAction(tr("Удалить", "Delete"), AppSymbol.Trash, canWrite && item.canDelete(session.files.spaces), danger = true,
                    run = { sheet = null; deleting = item }),
            )),
            SheetGroup(tr("Шифрование", "Encryption"), listOf(
                if (isEncrypted(item)) SheetAction(tr("Расшифровать (age)…", "Decrypt (age)…"), AppSymbol.Passwords,
                    canWrite && session.directWrite?.phase != DirectWritePhase.Unknown, run = { sheet = null; crypting = item to true })
                else SheetAction(tr("Зашифровать (age)…", "Encrypt (age)…"), AppSymbol.Vault,
                    canWrite && item.kind == EntryKind.File && session.directWrite?.phase != DirectWritePhase.Unknown,
                    run = { sheet = null; crypting = item to false }),
            )),
        ), onDismiss = { sheet = null })
    }
    batchCrypto?.let { (sources, decrypt) -> BatchCryptoDialog(session, sources, decrypt) { batchCrypto = null; selected = emptySet(); rescan = true } }
    crypting?.let { (source, decrypt) ->
        FileCryptoDialog(session, source, decrypt, initialParent = Location(source.location.storageId, source.location.path.substringBeforeLast('/', ""))) {
            crypting = null; rescan = true
        }
    }
    transferring?.let { (source, mode) ->
        TransferDestinationDialog(session.core, source, session.files.spaces, mode, onDismiss = { transferring = null },
            onTransfer = { parent, name, conflict, _ ->
                transferring = null
                session.transferFound(source, parent, name, mode, if (conflict == NameConflict.Replace) NameConflict.Fail else conflict)
                rescan = true
            })
    }
    renaming?.let { source ->
        RenameEntryDialog(source, onDismiss = { renaming = null }) { name -> renaming = null; session.renameFound(source, name); rescan = true }
    }
    deleting?.takeIf { session.trashAvailable(listOf(it)) && !permanent }?.let { source ->
        io.github.stratayun.paranoic.ui.screens.files.TrashConfirmDialog(1, source.name,
            onTrash = { deleting = null; session.moveToTrash(listOf(source)); rescan = true },
            onPermanent = { permanent = true }, onDismiss = { deleting = null })
    }
    deleting?.takeIf { !session.trashAvailable(listOf(it)) || permanent }?.let { source ->
        AlertDialog(onDismissRequest = { deleting = null; permanent = false }, title = { Text(tr("Удалить «${displayName(source.name)}»?", "Delete \"${displayName(source.name)}\"?")) },
            text = { Text(tr("Файл будет удалён без корзины. Отменить нельзя.", "The file is deleted without a trash. This cannot be undone.")) },
            confirmButton = { TextButton(enabled = canWrite, onClick = { deleting = null; permanent = false; session.deleteFound(source); rescan = true }) {
                Text(tr("Удалить", "Delete"), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { deleting = null; permanent = false }) { Text(tr("Отмена", "Cancel")) } })
    }
    trashingMany?.let { items ->
        if (session.trashAvailable(items)) TrashConfirmDialog(items.size, items.singleOrNull()?.name,
            onTrash = { trashingMany = null; selected = emptySet(); session.moveToTrash(items); rescan = true },
            onPermanent = { trashingMany = null; deleting = items.singleOrNull(); permanent = true },
            onDismiss = { trashingMany = null })
        else AlertDialog(onDismissRequest = { trashingMany = null }, title = { Text(tr("Удалить: ${items.size}?", "Delete ${items.size}?")) },
            text = { Text(tr("Корзина здесь недоступна: файлы будут удалены сразу.", "No trash here: files are deleted immediately.")) },
            confirmButton = { TextButton(onClick = { trashingMany = null; selected = emptySet(); items.forEach { session.deleteFound(it) }; rescan = true }) {
                Text(tr("Удалить", "Delete"), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { trashingMany = null }) { Text(tr("Отмена", "Cancel")) } })
    }
    info?.let { item ->
        AlertDialog(onDismissRequest = { info = null }, title = { Text(displayName(item.name)) }, text = { Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (item.sizeKnown) Text(tr("Размер: ", "Size: ") + humanSize(item.bytes))
            fileModifiedLabel(item.modifiedMs)?.let { Text(tr("Изменён: ", "Modified: ") + it) }
            Text(tr("Путь: /", "Path: /") + displayName(item.location.path))
        } }, confirmButton = { TextButton(onClick = { info = null }) { Text(tr("Закрыть", "Close")) } })
    }
}

private fun fileSymbolFor(category: FileCategory) = when (category) {
    FileCategory.Images -> AppSymbol.Photo; FileCategory.Videos -> AppSymbol.Video
    FileCategory.Music -> AppSymbol.Audio; FileCategory.Documents -> AppSymbol.Document
    FileCategory.All -> AppSymbol.Search; FileCategory.Archives -> AppSymbol.Archive
}

@Composable
private fun Thumbnail(core: CoreGateway, item: FileItem, pngImage: @Composable (ByteArray, Modifier) -> Unit, modifier: Modifier) {
    var png by remember(item.location, item.modifiedMs) { mutableStateOf<ByteArray?>(null) }
    LaunchedEffect(core, item.location, item.modifiedMs) {
        (core.imagePng(item.location, 256) as? CoreResult.Value)?.let { png = it.value }
    }
    DisposableEffect(item.location) { onDispose { png?.fill(0); png = null } }
    Box(modifier, contentAlignment = Alignment.Center) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surfaceContainerHigh) {}
        val bytes = png
        if (bytes != null) pngImage(bytes, Modifier.fillMaxSize().clearAndSetSemantics { })
        else AppIcon(AppSymbol.Photo)
    }
}

/** Sort order of a category list. */
enum class CategorySort(val comparator: Comparator<FileItem>) {
    Newest(compareByDescending { it.modifiedMs ?: 0uL }),
    Oldest(compareBy { it.modifiedMs ?: 0uL }),
    Name(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name }),
    Largest(compareByDescending { it.bytes }),
    Smallest(compareBy { it.bytes }),
    Folder(compareBy<FileItem, String>(String.CASE_INSENSITIVE_ORDER) { it.location.path.substringBeforeLast('/', "") }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name }),
}

@Composable
fun CategorySort.label(): String = when (this) {
    CategorySort.Newest -> tr("Сначала новые", "Newest first")
    CategorySort.Oldest -> tr("Сначала старые", "Oldest first")
    CategorySort.Name -> tr("По имени", "By name")
    CategorySort.Largest -> tr("Сначала большие", "Largest first")
    CategorySort.Smallest -> tr("Сначала маленькие", "Smallest first")
    CategorySort.Folder -> tr("По папкам", "By folder")
}

/** Ringtones, notification and alarm sounds, UI sounds and messenger voice notes are not music. */
private val systemSoundFolders = setOf("ringtones", "notifications", "alarms", "ui", "voice notes", "voicenotes", "audiorecord")
internal fun isSystemSound(item: FileItem): Boolean =
    item.location.path.lowercase().split('/').dropLast(1).any { it in systemSoundFolders }
