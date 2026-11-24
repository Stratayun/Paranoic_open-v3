package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*

/** Modal, memory-only document scope. No OS path, exported grant or external viewer. */
@Composable
internal fun VaultFileWindow(session: AppSession, gateway: EncryptedFolderGateway, handle: String,
    path: List<String>, entry: VaultEntry, writable: Boolean,
    secureImage: @Composable (ByteArray, Modifier) -> Unit,
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit,
    onBusy: (Boolean) -> Unit, onClose: () -> Unit) {
    key(handle, path, entry.name) {
        val guard = remember { DocumentExitGuard() }
        val close = { guard.request(onClose) }
        DisposableEffect(guard) { onDispose { guard.busy = false; guard.cancel(); onBusy(false) } }
        Dialog(onDismissRequest = close, properties = DialogProperties(usePlatformDefaultWidth = false,
            dismissOnBackPress = true, dismissOnClickOutside = false)) {
            Surface(Modifier.fillMaxSize().trackSecretActivity()) {
                Column(Modifier.fillMaxSize()) {
                    Row(Modifier.fillMaxWidth()) {
                        TextButton(onClick = { session.pause(); session.resume() }) { Text(tr("Заблокировать сейчас", "Lock now")) }
                    }
                    Box(Modifier.weight(1f).fillMaxWidth()) {
                when (entry.name.substringAfterLast('.', "").lowercase()) {
                    "txt", "md", "markdown" -> VaultTextPane(session, gateway, handle, path, entry,
                        writable, guard, onBusy, close)
                    "zip", "7z" -> VaultArchivePane(session, gateway, handle, path, entry,
                        writable, guard, onBusy, close)
                    "png", "jpg", "jpeg", "gif", "webp" -> VaultImagePane(gateway, handle, path, entry, secureImage, close)
                    "pdf", "docx", "xlsx", "ods", "mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "mp4", "mkv", "webm", "mov" ->
                        VaultPreviewPane(session.core, gateway, handle, path, entry, secureImage, mediaPlayer, close)
                    else -> Column(Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        TextButton(onClick = close) { Text(tr("Назад", "Back")) }
                        Text(displayName(entry.name), Modifier.sectionHeading())
                        FaultNotice(CoreFault.Unsupported)
                    }
                }
                    }
                }
            }
            if (guard.pending != null) AlertDialog(onDismissRequest = guard::cancel,
                title = { Text(tr("Закрыть черновик?", "Close the draft?")) },
                text = { Text(tr("Несохранённые правки пропадут. Уже созданные копии и частичные результаты операций останутся в хранилище.",
                    "Unsaved edits are lost. Copies and partial operation results already created remain in the vault.")) },
                confirmButton = { TextButton(enabled = !guard.busy, onClick = guard::discard) { Text(tr("Закрыть", "Close")) } },
                dismissButton = { TextButton(onClick = guard::cancel) { Text(tr("Остаться", "Stay")) } })
        }
    }
}
