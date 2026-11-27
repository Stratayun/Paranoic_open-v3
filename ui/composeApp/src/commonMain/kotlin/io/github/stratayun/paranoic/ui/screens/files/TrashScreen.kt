package io.github.stratayun.paranoic.ui.screens.files

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.launch
import io.github.stratayun.paranoic.resources.*

/**
 * Trash of one device place: restore puts an entry back into its folder
 * (never over an existing file); "delete forever" and "empty" erase.
 */
@Composable
fun TrashScreen(session: AppSession, space: Space, onClose: () -> Unit) {
    val core = session.core
    val scope = rememberCoroutineScope()
    var items by remember(space) { mutableStateOf<List<TrashItem>?>(null) }
    var fault by remember(space) { mutableStateOf<CoreFault?>(null) }
    var busy by remember(space) { mutableStateOf(false) }
    var revision by remember(space) { mutableStateOf(0) }
    var purge by remember(space) { mutableStateOf<TrashItem?>(null) }
    var emptyAll by remember(space) { mutableStateOf(false) }
    LaunchedEffect(space, revision) {
        when (val listed = core.trashList(space)) {
            is CoreResult.Value -> { items = listed.value; fault = null }
            is CoreResult.Failure -> fault = listed.fault
        }
    }
    fun act(block: suspend () -> CoreResult<*>) {
        busy = true
        scope.launch {
            val result = block()
            fault = (result as? CoreResult.Failure)?.fault
            busy = false; revision++
            session.refresh(); session.refreshTrash()
        }
    }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose, modifier = Modifier.actionDescription(tr("Назад", "Back"))) { AppIcon(AppSymbol.Back) }
                    Column(Modifier.weight(1f)) {
                        Text(tr("Корзина", "Trash"), style = MaterialTheme.typography.headlineSmall)
                        Text(displayName(space.name), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    TextButton(enabled = !busy && !items.isNullOrEmpty(), onClick = { emptyAll = true }) {
                        Text(tr("Очистить", "Empty"), color = MaterialTheme.colorScheme.error)
                    }
                }
                fault?.let { FaultNotice(it) }
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
                when {
                    items == null && fault == null -> LinearProgressIndicator(Modifier.fillMaxWidth())
                    items.isNullOrEmpty() -> Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            BrandIcon(Res.drawable.ic_trash_empty, size = 112.dp)
                            Text(tr("Корзина пуста", "Trash is empty"), style = MaterialTheme.typography.titleMedium)
                        }
                    }
                    else -> LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        val list = items.orEmpty()
                        items(list, key = { it.bucket }) { item ->
                            val position = groupPosition(list.indexOf(item), list.size)
                            GroupedRow(displayName(item.name), position,
                                subtitle = (if (item.originalFolder.isEmpty()) tr("из корня", "from root") else tr("из ", "from ") + item.originalFolder) +
                                    if (item.folder) "" else " · " + humanSize(item.bytes.toULong()),
                                leading = { AppIcon(if (item.folder) AppSymbol.Folder else AppSymbol.Document) },
                                trailing = {
                                    Row {
                                        TextButton(enabled = !busy, onClick = { act { core.trashRestore(space, item.bucket) } }) { Text(tr("Вернуть", "Restore")) }
                                        IconButton(enabled = !busy, onClick = { purge = item },
                                            modifier = Modifier.actionDescription(tr("Удалить навсегда", "Delete forever"))) { AppIcon(AppSymbol.Trash) }
                                    }
                                })
                        }
                    }
                }
                Text(tr("Файлы лежат в скрытой папке .Trash-Paranoic этой памяти и занимают место, пока корзина не очищена.",
                    "Files stay in the hidden .Trash-Paranoic folder of this storage and use space until the trash is emptied."),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    purge?.let { item ->
        AlertDialog(onDismissRequest = { purge = null }, title = { Text(tr("Удалить навсегда?", "Delete forever?")) },
            text = { Text(tr("«${displayName(item.name)}» будет удалён без возможности вернуть.", "\"${displayName(item.name)}\" will be deleted permanently.")) },
            confirmButton = { TextButton(onClick = { purge = null; act { core.trashPurge(space, item.bucket) } }) {
                Text(tr("Удалить", "Delete"), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { purge = null }) { Text(tr("Отмена", "Cancel")) } })
    }
    if (emptyAll) AlertDialog(onDismissRequest = { emptyAll = false }, title = { Text(tr("Очистить корзину?", "Empty the trash?")) },
        text = { Text(tr("Все файлы в корзине будут удалены без возможности вернуть.", "Every file in the trash will be deleted permanently.")) },
        confirmButton = { TextButton(onClick = { emptyAll = false; act { core.trashEmpty(space) } }) {
            Text(tr("Очистить", "Empty"), color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = { emptyAll = false }) { Text(tr("Отмена", "Cancel")) } })
}

/** Delete prompt where the trash is available: trash by default, permanent deletion on request. */
@Composable
fun TrashConfirmDialog(count: Int, name: String?, onTrash: () -> Unit, onPermanent: (() -> Unit)?, onDismiss: () -> Unit) {
    AlertDialog(onDismissRequest = onDismiss,
        title = { Text(if (name != null) tr("Удалить «${displayName(name)}»?", "Delete \"${displayName(name)}\"?") else tr("Удалить объекты: $count?", "Delete $count items?")) },
        text = { Text(tr("Файлы будут перемещены в корзину этой памяти — их можно вернуть.", "Items go to this storage's trash and can be restored.")) },
        confirmButton = { TextButton(onClick = onTrash) { Text(tr("В корзину", "Move to trash")) } },
        dismissButton = {
            Row {
                if (onPermanent != null) TextButton(onClick = onPermanent) { Text(tr("Навсегда…", "Forever…"), color = MaterialTheme.colorScheme.error) }
                TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) }
            }
        })
}
