package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
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
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun FileCategory.label(): String = when (this) {
    FileCategory.Images -> tr("Изображения", "Pictures")
    FileCategory.Videos -> tr("Фильмы", "Movies")
    FileCategory.Music -> tr("Музыка", "Music")
    FileCategory.Documents -> tr("Документы", "Documents")
}

/** All files of one kind in a place (drawer quick places). Tap opens; long press shows where it is. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CategoryScreen(session: AppSession, state: CategoryState, onOpenFile: (FileItem) -> Unit,
    pngImage: @Composable (ByteArray, Modifier) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    var sheet by remember(state.category) { mutableStateOf<FileItem?>(null) }
    var info by remember(state.category) { mutableStateOf<FileItem?>(null) }
    fun showInFolder(item: FileItem) {
        val parent = item.location.path.substringBeforeLast('/', "")
        session.categories.close()
        session.openPath(state.space, parent)
    }
    Column(Modifier.fillMaxSize().padding(horizontal = 12.dp)) {
        Surface(Modifier.fillMaxWidth().padding(vertical = 8.dp), shape = MaterialTheme.shapes.extraLarge, color = scheme.surfaceContainer) {
            Row(Modifier.padding(start = 4.dp, end = 12.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = session.categories::close, modifier = Modifier.actionDescription(tr("Закрыть", "Close"))) { AppIcon(AppSymbol.Back) }
                Column(Modifier.weight(1f)) {
                    Text(state.category.label(), style = MaterialTheme.typography.titleMedium)
                    Text(displayName(state.space.name) + " · " + tr("найдено ${state.items.size}", "${state.items.size} found") +
                        if (state.running) tr(" · ищем…", " · searching…") else if (state.truncated) tr(" · показаны первые", " · partial") else "",
                        style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
            }
        }
        if (state.running) LinearProgressIndicator(Modifier.fillMaxWidth())
        state.fault?.let { FaultNotice(it) }
        if (!state.running && state.items.isEmpty() && state.fault == null) Column(Modifier.fillMaxWidth().padding(top = 48.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            ShapedIcon(fileSymbolFor(state.category), ExpressiveShapes.Sunny, scheme.surfaceContainerHighest, scheme.onSurfaceVariant, size = 96.dp)
            Text(tr("Ничего не найдено", "Nothing found"), style = MaterialTheme.typography.titleMedium)
        }
        if (state.category == FileCategory.Images) LazyVerticalGrid(GridCells.Adaptive(112.dp), Modifier.weight(1f),
            horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            items(state.items, key = { it.location.storageId + ":" + it.location.path }) { item ->
                Box(Modifier.aspectRatio(1f).clip(MaterialTheme.shapes.medium)
                    .combinedClickable(onLongClick = { sheet = item }, onClick = { onOpenFile(item) })) {
                    Thumbnail(session.core, item, pngImage, Modifier.fillMaxSize())
                }
            }
        } else LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            items(state.items, key = { it.location.storageId + ":" + it.location.path }) { item ->
                Row(Modifier.fillMaxWidth().clip(MaterialTheme.shapes.large).combinedClickable(onLongClick = { sheet = item }, onClick = { onOpenFile(item) })
                    .heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    ShapedIcon(fileSymbol(item), MaterialTheme.shapes.medium, scheme.surfaceContainerHighest, scheme.onSurfaceVariant)
                    Column(Modifier.weight(1f)) {
                        Text(displayName(item.name), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("/" + displayName(item.location.path.substringBeforeLast('/', "")) + " · " +
                            (if (item.sizeKnown) humanSize(item.bytes) else ""), style = MaterialTheme.typography.bodySmall,
                            color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    IconButton(onClick = { sheet = item }, modifier = Modifier.actionDescription(tr("Действия", "Actions"))) { AppIcon(AppSymbol.More) }
                }
            }
        }
    }
    sheet?.let { item ->
        FileActionSheet(item.name, "/" + displayName(item.location.path.substringBeforeLast('/', "")), fileSymbol(item), listOf(
            SheetGroup(tr("Действия", "Actions"), listOf(
                SheetAction(tr("Открыть", "Open"), AppSymbol.Forward, run = { sheet = null; onOpenFile(item) }),
                SheetAction(tr("Показать в папке", "Show in folder"), AppSymbol.Folder, run = { sheet = null; showInFolder(item) }),
                SheetAction(tr("Свойства", "Properties"), AppSymbol.Info, run = { sheet = null; info = item }),
            )),
        ), onDismiss = { sheet = null })
    }
    info?.let { item ->
        AlertDialog(onDismissRequest = { info = null }, title = { Text(displayName(item.name)) }, text = { Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (item.sizeKnown) Text(tr("Размер: ", "Size: ") + humanSize(item.bytes))
            fileModifiedLabel(item.modifiedMs)?.let { Text(tr("Изменён: ", "Modified: ") + it) }
            Text(tr("Путь: /", "Path: /") + displayName(item.location.path))
            Text(tr("Переместить, переименовать или удалить можно через «Показать в папке».", "Use Show in folder to move, rename or delete."),
                style = MaterialTheme.typography.bodySmall)
        } }, confirmButton = { TextButton(onClick = { info = null }) { Text(tr("Закрыть", "Close")) } })
    }
}

private fun fileSymbolFor(category: FileCategory) = when (category) {
    FileCategory.Images -> AppSymbol.Photo; FileCategory.Videos -> AppSymbol.Video
    FileCategory.Music -> AppSymbol.Audio; FileCategory.Documents -> AppSymbol.Document
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
