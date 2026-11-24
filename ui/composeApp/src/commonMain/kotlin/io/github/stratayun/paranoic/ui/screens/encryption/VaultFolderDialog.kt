package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

@Composable
internal fun VaultFolderDialog(gateway: EncryptedFolderGateway, handle: String, initial: List<String>,
    onDismiss: () -> Unit, onChoose: (List<String>) -> Unit) {
    var path by remember { mutableStateOf(initial) }
    var listing by remember { mutableStateOf<VaultListing?>(null) }
    var listed by remember { mutableStateOf<List<String>?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var loading by remember { mutableStateOf(true) }
    var retry by remember { mutableIntStateOf(0) }
    LaunchedEffect(gateway, handle, path, retry) {
        listing = null; listed = null; fault = null; loading = true
        try {
            val result = gateway.list(handle, path)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> { listing = result.value; listed = path.toList() }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { if (currentCoroutineContext().isActive) loading = false }
    }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss,
        title = { Text(tr("Папка внутри хранилища", "Folder inside the vault")) },
        text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(vaultPath(path), maxLines = 3)
            TextButton(enabled = path.isNotEmpty() && !loading, onClick = { path = path.dropLast(1) }) { Text(tr("Выше", "Up")) }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it); TextButton(onClick = { retry++ }) { Text(tr("Перечитать", "Reload")) } }
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 220.dp)) {
                items(listing?.entries.orEmpty().filter { it.folder }, key = { it.name }) { entry ->
                    TextButton(enabled = !loading && path.size < 64, onClick = { path = path + entry.name }) { Text(displayName(entry.name)) }
                }
            }
        } },
        confirmButton = { TextButton(enabled = !loading && listed == path && listing != null && fault == null,
            onClick = { if (!loading && listed == path && listing != null && fault == null) onChoose(path.toList()) }) { Text(tr("Выбрать эту папку", "Choose this folder")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
