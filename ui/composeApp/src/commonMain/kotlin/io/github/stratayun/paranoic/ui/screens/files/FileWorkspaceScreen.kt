package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

private data class DraggedFile(val tabId: String, val item: FileItem, val position: Offset)
private data class DroppedFile(val sourceTab: String, val source: FileItem, val targetTab: String, val parent: Location)

/** Each pane renders a fixed tab ID. Only explicit actions change the active pane. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FileWorkspaceScreen(session: AppSession, defaultView: FileView, onOpenFile: (FileItem) -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit, onOpenPlaces: (() -> Unit)? = null, onNewVault: (() -> Unit)? = null) {
    val workspace = session.fileWorkspace
    var menu by remember(session.secretsEpoch) { mutableStateOf(false) }
    var dragged by remember(session.secretsEpoch) { mutableStateOf<DraggedFile?>(null) }
    var dropped by remember(session.secretsEpoch) { mutableStateOf<DroppedFile?>(null) }
    var feedback by remember(session.secretsEpoch) { mutableStateOf<String?>(null) }
    val bounds = remember(session.secretsEpoch) { mutableStateMapOf<String, Rect>() }
    val clipboard = remember(session.secretsEpoch) { FileClipboard() }
    // Open device storage on first entry instead of an empty "pick a place" screen.
    val spaces = session.files.spaces
    var autoOpened by remember(session.secretsEpoch) { mutableStateOf(false) }
    LaunchedEffect(spaces, autoOpened) {
        if (autoOpened || session.files.trail.isNotEmpty()) return@LaunchedEffect
        val device = spaces.firstOrNull { it.detachable && !it.cloud && !it.androidTree } ?: return@LaunchedEffect
        autoOpened = true
        session.openSpace(device)
    }
    val chooseFolder = tr("Сначала откройте доступную для записи папку в другой панели.", "First open a writable folder in the other pane.")
    fun prepareCopy(sourceTab: String, item: FileItem) {
        val now = session.fileWorkspace
        val target = if (sourceTab == now.left) now.right else now.left
        val destination = target?.let { now.tab(it)?.files }
        val parent = destination?.folder
        if (target == null || destination == null || parent == null || !destination.loaded || destination.loading ||
            !item.canTransferTo(parent, TransferMode.Copy, destination.spaces)) feedback = chooseFolder
        else { dropped = DroppedFile(sourceTab, item, target, parent); feedback = null }
    }
    fun dragHandler(sourceTab: String) = FileDragHandler(start = { item, point ->
        if (!session.writing) { dragged = DraggedFile(sourceTab, item, point); feedback = null }
    }, move = { amount -> dragged = dragged?.let { it.copy(position = it.position + amount) } }, end = {
        val current = dragged
        dragged = null
        if (current != null) {
            val now = session.fileWorkspace
            val target = if (current.tabId == now.left) now.right else now.left
            if (target != null && bounds[target]?.contains(current.position) == true) prepareCopy(current.tabId, current.item)
        }
    }, cancel = { dragged = null }, copyToOtherPane = { item -> prepareCopy(sourceTab, item) })
    val activeIndex = workspace.tabs.indexOfFirst { it.id == workspace.active }
    session.categories.state?.let { category ->
        CategoryScreen(session, category, onOpenFile, pngImage, clipboard)
        return
    }
    Column(Modifier.fillMaxSize()) {
        if (workspace.tabs.size > 1) LazyRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp), contentPadding = PaddingValues(horizontal = 12.dp)) {
            items(workspace.tabs, key = { it.id }) { tab ->
                val title = tab.files.trail.lastOrNull()?.name ?: tr("Места", "Places")
                FilterChip(selected = workspace.active == tab.id, onClick = { session.activateFileTab(tab.id) },
                    label = { Text(displayName(title), maxLines = 1) },
                    trailingIcon = { IconButton(onClick = { session.closeFileTab(tab.id) }, modifier = Modifier.size(24.dp)
                        .actionDescription(tr("Закрыть вкладку", "Close tab"))) { AppIcon(AppSymbol.Close, Modifier.size(16.dp)) } })
            }
        }
        feedback?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        dragged?.let { Text(tr("Перетащите в другую панель: ", "Drop onto the other pane: ") + displayName(it.item.name), Modifier.padding(horizontal = 12.dp)) }
        BoxWithConstraints(Modifier.weight(1f)) {
            val two = workspace.right != null && maxWidth >= 720.dp
            if (workspace.right != null && !two) Column(Modifier.fillMaxSize()) {
                Pane(session, workspace.active, defaultView, onOpenFile, pngImage, Modifier.weight(1f), onOpenPlaces = onOpenPlaces, clipboard = clipboard, onNewVault = onNewVault)
            } else if (two) Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pane(session, workspace.left, defaultView, onOpenFile, pngImage,
                    Modifier.weight(1f).onGloballyPositioned { bounds[workspace.left] = it.boundsInRoot() }, dragHandler(workspace.left), onOpenPlaces, clipboard, onNewVault)
                val right = requireNotNull(workspace.right)
                Pane(session, right, defaultView, onOpenFile, pngImage,
                    Modifier.weight(1f).onGloballyPositioned { bounds[right] = it.boundsInRoot() }, dragHandler(right), onOpenPlaces, clipboard, onNewVault)
            } else Pane(session, workspace.active, defaultView, onOpenFile, pngImage, Modifier.fillMaxSize(), onOpenPlaces = onOpenPlaces, clipboard = clipboard, onNewVault = onNewVault)
        }
    }
    dropped?.let { request ->
        var conflict by remember(request) { mutableStateOf(NameConflict.Fail) }
        val destination = session.fileWorkspace.tab(request.targetTab)?.files
        val collision = destination?.entries?.any { it.name == request.source.name } == true
        AlertDialog(onDismissRequest = { dropped = null }, title = { Text(tr("Копировать в другую панель?", "Copy to the other pane?")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(request.source.name))
                Text(tr("Назначение: /", "Destination: /") + displayName(request.parent.path))
                Text(tr("Исходник сохраняется. Замена существующего файла отключена.", "The source is kept. Replacing an existing file is disabled."))
                FilterChip(selected = conflict == NameConflict.KeepBoth, onClick = { conflict = if (conflict == NameConflict.KeepBoth) NameConflict.Fail else NameConflict.KeepBoth },
                    label = { Text(tr("При совпадении сохранить оба", "Keep both if the name exists")) })
                if (collision && conflict == NameConflict.Fail) Text(tr("Имя занято. Включите сохранение обоих или выберите другую папку.", "The name exists. Enable Keep both or choose another folder."))
            } }, confirmButton = { TextButton(enabled = !session.writing && destination?.loaded == true && !destination.loading &&
                destination.folder == request.parent && (!collision || conflict == NameConflict.KeepBoth), onClick = {
                    dropped = null
                    session.copyBetweenFileTabs(request.sourceTab, request.source, request.targetTab, request.parent, conflict)
                }) { Text(tr("Копировать", "Copy")) } }, dismissButton = { TextButton(onClick = { dropped = null }) { Text(tr("Отмена", "Cancel")) } })
    }
}

@Composable
private fun Pane(session: AppSession, tabId: String, defaultView: FileView, onOpenFile: (FileItem) -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit, modifier: Modifier, drag: FileDragHandler? = null, onOpenPlaces: (() -> Unit)? = null,
    clipboard: FileClipboard? = null, onNewVault: (() -> Unit)? = null) {
    val tab = session.fileWorkspace.tab(tabId) ?: return
    Surface(modifier.fillMaxHeight().onFocusChanged { if (it.hasFocus) session.activateFileTab(tabId) }.focusGroup()
        .pointerInput(tabId) {
            awaitPointerEventScope { while (true) {
                val event = awaitPointerEvent(PointerEventPass.Initial)
                if (event.changes.any { it.pressed && !it.previousPressed }) session.activateFileTab(tabId)
            } }
        }, shape = MaterialTheme.shapes.large, border = if (session.fileWorkspace.right == null) null else BorderStroke(1.dp, if (session.fileWorkspace.active == tabId)
        MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant)) {
        key(tabId, session.secretsEpoch) {
            FilesScreen(session, tab.view ?: defaultView, {}, onOpenFile, pngImage, tabId, drag, onOpenPlaces, clipboard, onNewVault)
        }
    }
}
