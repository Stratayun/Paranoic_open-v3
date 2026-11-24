package io.github.stratayun.paranoic.ui.screens.encryption

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException

/** Select only authenticated folders of the SAME open 1.8 vault. Rust revalidates the move. */
@Composable
internal fun VaultMoveDialog(gateway: EncryptedFolderGateway, handle: String, sourcePath: List<String>, entry: VaultEntry,
    onClose: () -> Unit, onMove: (List<String>) -> Unit) {
    var target by remember { mutableStateOf(sourcePath) }
    var folders by remember { mutableStateOf<List<VaultEntry>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    val excluded = if (entry.folder) sourcePath + entry.name else null
    fun allowed(path: List<String>) = path != sourcePath && (excluded == null || path.take(excluded.size) != excluded)
    LaunchedEffect(handle, target, retry) {
        loading = true
        folders = emptyList()
        fault = null
        try {
            when (val result = gateway.list(handle, target)) {
                is CoreResult.Value -> folders = result.value.entries.filter { child ->
                    child.folder && (excluded == null || (target + child.name).take(excluded.size) != excluded)
                }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { loading = false }
    }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onClose, title = { Text(tr("Перенести внутри папки 1.8", "Move inside the 1.8 vault")) },
        text = { Column(Modifier.fillMaxWidth().heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Объект: ", "Entry: ") + vaultPath(sourcePath + entry.name))
            Text(tr("В папку: ", "To folder: ") + vaultPath(target))
            Text(tr("Имя сохранится. Существующий объект не заменяется. Папку нельзя перенести в неё саму или внутрь её подпапок.",
                "The name stays unchanged. Existing entries are not replaced. A folder cannot be moved into itself or its descendants."),
                style = MaterialTheme.typography.bodySmall)
            if (target.isNotEmpty()) TextButton(enabled = !loading, onClick = { loading = true; target = target.dropLast(1) }) { Text(tr("На уровень выше", "Up one level")) }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let {
                FaultNotice(it)
                TextButton(onClick = { retry++ }) { Text(tr("Повторить чтение", "Retry reading")) }
            }
            LazyColumn(Modifier.heightIn(max = 220.dp).fillMaxWidth()) {
                items(folders, key = { it.name }) { folder ->
                    TextButton(enabled = !loading, onClick = { loading = true; target = target + folder.name }, modifier = Modifier.fillMaxWidth()) {
                        Text(displayName(folder.name))
                    }
                }
            }
        } },
        confirmButton = { TextButton(enabled = !loading && fault == null && allowed(target), onClick = {
            if (!loading && fault == null && allowed(target)) onMove(target.toList())
        }) {
            Text(tr("Перенести сюда", "Move here"))
        } },
        dismissButton = { TextButton(onClick = onClose) { Text(tr("Отмена", "Cancel")) } })
}

internal fun vaultPath(path: List<String>): String = "/" + path.joinToString("/") { displayName(it) }
