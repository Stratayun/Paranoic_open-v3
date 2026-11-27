package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
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
    pngImage: @Composable (ByteArray, Modifier) -> Unit, onOpenPlaces: (() -> Unit)? = null, onNewVault: (() -> Unit)? = null,
    onSendToSafe: ((List<FileItem>) -> Unit)? = null) {
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
        val device = spaces.firstOrNull { it.detachable && !it.cloud } ?: return@LaunchedEffect
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
        CategoryScreen(session, category, onOpenFile, pngImage, clipboard, onSendToSafe)
        return
    }
    Column(Modifier.fillMaxSize()) {
        FileTabStrip(session)
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

/**
 * Tabs like a desktop file explorer (owner 26.11): always visible, "+" opens a new tab
 * on the same folder, the cross closes one (the last tab stays).
 */
@Composable
private fun FileTabStrip(session: AppSession) {
    val workspace = session.fileWorkspace
    val scheme = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 4.dp, top = 2.dp), verticalAlignment = Alignment.Bottom) {
        LazyRow(Modifier.weight(1f, fill = false), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            items(workspace.tabs, key = { it.id }) { tab ->
                val on = workspace.active == tab.id
                val title = tab.files.trail.lastOrNull()?.name ?: tr("Места", "Places")
                Surface(onClick = { session.activateFileTab(tab.id) },
                    shape = RoundedCornerShape(topStart = 10.dp, topEnd = 10.dp),
                    color = if (on) scheme.surfaceContainerHighest else scheme.surfaceContainerLow,
                    contentColor = if (on) scheme.onSurface else scheme.onSurfaceVariant) {
                    Column(Modifier.widthIn(min = 96.dp, max = 200.dp)) {
                        Row(Modifier.height(36.dp).padding(start = 12.dp, end = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                            AppIcon(AppSymbol.Folder, Modifier.size(16.dp))
                            Spacer(Modifier.width(6.dp))
                            Text(displayName(title), Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.labelLarge)
                            if (workspace.tabs.size > 1) IconButton(onClick = { session.closeFileTab(tab.id) }, modifier = Modifier.size(28.dp)
                                .actionDescription(tr("Закрыть вкладку", "Close tab"))) { AppIcon(AppSymbol.Close, Modifier.size(14.dp)) }
                            else Spacer(Modifier.width(10.dp))
                        }
                        Box(Modifier.fillMaxWidth().height(2.dp).background(if (on) scheme.primary else Color.Transparent))
                    }
                }
            }
        }
        IconButton(onClick = { session.newFileTab(duplicate = true) }, modifier = Modifier.size(36.dp)
            .actionDescription(tr("Новая вкладка", "New tab"))) { AppIcon(AppSymbol.Add, Modifier.size(20.dp)) }
    }
    HorizontalDivider(color = scheme.outlineVariant.copy(alpha = 0.5f))
}
