package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.CoreFault
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
@OptIn(ExperimentalLayoutApi::class)
fun ArchiveViewer(session: AppSession, onClose: () -> Unit) {
    val archive = session.archives
    val state = archive.state
    var action by remember(session.secretsEpoch) { mutableStateOf<ArchiveAction?>(null) }
    LaunchedEffect(state.source?.location, state.fault) {
        if (state.fault == CoreFault.PasswordRequired || state.fault == CoreFault.WrongPassword)
            action = ArchiveAction.Open
    }
    DisposableEffect(archive) { onDispose { archive.close() } }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(displayName(state.source?.name.orEmpty()), Modifier.sectionHeading(), style = MaterialTheme.typography.titleLarge)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(enabled = !session.writing && !state.loading, onClick = { action = ArchiveAction.Open }) { Text(tr("Формат / пароль…", "Format / password…")) }
            TextButton(enabled = !session.writing && !state.loading && state.loaded, onClick = { action = ArchiveAction.Extract }) { Text(tr("Распаковать…", "Extract…")) }
            TextButton(enabled = !session.writing, onClick = { archive.close(); onClose() }) { Text(tr("Закрыть", "Close")) }
        }
        if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        state.fault?.let { fault -> FaultNotice(fault, message = if (fault == CoreFault.WrongPassword &&
            state.format == io.github.stratayun.paranoic.bridge.ArchiveFormat.SevenZip)
            tr("Неверный пароль или повреждённые зашифрованные данные 7z.", "Wrong password or damaged encrypted 7z data.")
            else fault.label()) }
        if (state.loaded) Text(tr("Записей: ${state.entries.size}", "Entries: ${state.entries.size}"))
        if (state.loaded && state.entries.isEmpty()) Text(tr("Архив пуст. Вернитесь к файлам, чтобы открыть другой архив.", "This archive is empty. Return to Files to open another archive."))
        LazyColumn(Modifier.weight(1f)) {
            // Duplicate archive member names must not cause duplicate Compose keys.
            itemsIndexed(state.entries, key = { index, _ -> index }) { _, entry ->
                Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                    Text((if (entry.folder) tr("Папка: ", "Folder: ") else "") + displayName(entry.path))
                    if (entry.encrypted) Text(tr("Зашифровано · для распаковки нужен пароль", "Encrypted · extraction requires a password"),
                        style = MaterialTheme.typography.bodySmall)
                    if (!entry.folder) Text(tr("Размер: ${entry.size} байт · в архиве: ${entry.compressed} байт",
                        "Size: ${entry.size} B · packed: ${entry.compressed} B"), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
    action?.let { selected -> state.source?.let { source ->
        ArchiveDialog(session, listOf(source), selected, onDismiss = { action = null })
    } }
}
