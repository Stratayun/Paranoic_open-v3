package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.CoreFault
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun ArchiveViewer(session: AppSession, startExtract: Boolean = false, onClose: () -> Unit) {
    val archive = session.archives
    val state = archive.state
    val scheme = MaterialTheme.colorScheme
    var action by remember(session.secretsEpoch) { mutableStateOf<ArchiveAction?>(null) }
    var extractRequested by remember(session.secretsEpoch) { mutableStateOf(startExtract) }
    LaunchedEffect(state.source?.location, state.fault) {
        if (state.fault == CoreFault.PasswordRequired || state.fault == CoreFault.WrongPassword)
            action = ArchiveAction.Open
    }
    // "Extract…" from the file sheet: show the destination dialog as soon as the listing is ready.
    LaunchedEffect(state.loaded, extractRequested) {
        if (extractRequested && state.loaded && action == null) { extractRequested = false; action = ArchiveAction.Extract }
    }
    DisposableEffect(archive) { onDispose { archive.close() } }
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Surface(color = scheme.surfaceContainer) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(enabled = !session.writing, onClick = { archive.close(); onClose() },
                    modifier = Modifier.actionDescription(tr("Закрыть архив", "Close archive"))) { AppIcon(AppSymbol.Back) }
                Column(Modifier.weight(1f)) {
                    Text(displayName(state.source?.name.orEmpty()), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(if (state.loaded) tr("Записей: ${state.entries.size}", "Entries: ${state.entries.size}") else tr("Открываем…", "Opening…"),
                        style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
                IconButton(enabled = !session.writing && !state.loading, onClick = { action = ArchiveAction.Open },
                    modifier = Modifier.actionDescription(tr("Формат и пароль", "Format and password"))) { AppIcon(AppSymbol.Passwords) }
            }
        }
        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(enabled = !session.writing && !state.loading && state.loaded, onClick = { action = ArchiveAction.Extract },
                modifier = Modifier.fillMaxWidth().height(56.dp)) {
                AppIcon(AppSymbol.Download); Spacer(Modifier.width(8.dp)); Text(tr("Извлечь всё…", "Extract all…"))
            }
            Text(tr("Распаковка создаёт новую папку; место можно выбрать.", "Extraction creates a new folder; you can pick where."),
                style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            state.fault?.let { fault -> FaultNotice(fault, message = if (fault == CoreFault.WrongPassword &&
                state.format == io.github.stratayun.paranoic.bridge.ArchiveFormat.SevenZip)
                tr("Неверный пароль или повреждённые зашифрованные данные 7z.", "Wrong password or damaged encrypted 7z data.")
                else fault.label()) }
            if (state.loaded && state.entries.isEmpty()) Text(tr("Архив пуст.", "This archive is empty."), color = scheme.onSurfaceVariant)
        }
        LazyColumn(Modifier.weight(1f).padding(horizontal = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            // Duplicate archive member names must not cause duplicate Compose keys.
            itemsIndexed(state.entries, key = { index, _ -> index }) { _, entry ->
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 8.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    ShapedIcon(if (entry.folder) AppSymbol.Folder else if (entry.encrypted) AppSymbol.Vault else AppSymbol.Document,
                        MaterialTheme.shapes.medium, if (entry.folder) scheme.primaryContainer else scheme.surfaceContainerHighest,
                        if (entry.folder) scheme.onPrimaryContainer else scheme.onSurfaceVariant, size = 40.dp)
                    Column(Modifier.weight(1f)) {
                        Text(displayName(entry.path), style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        if (!entry.folder) Text(tr("${entry.size} Б · сжато ${entry.compressed} Б", "${entry.size} B · packed ${entry.compressed} B") +
                            if (entry.encrypted) tr(" · с паролем", " · password") else "",
                            style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
    action?.let { selected -> state.source?.let { source ->
        ArchiveDialog(session, listOf(source), selected, onDismiss = { action = null })
    } }
}
