package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.FileItem
import io.github.stratayun.paranoic.ui.state.*

/** Confirmation only. Copy verification and conditional source removal belong to Rust. */
@Composable
internal fun VaultImportMoveDialog(files: List<FileItem>, enabled: Boolean, onClose: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onClose,
        title = { Text(tr("Переместить в хранилище / Сейф?", "Move into the vault / Safe?")) },
        text = { Column(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            files.forEach { Text(displayName(it.name)) }
            Text(tr("После проверки зашифрованной копии приложение попытается удалить именно прочитанную версию исходника. На SAF и Windows исходник остаётся: это будет копирование. Если исходник изменился или удалить его безопасно нельзя, он также сохранится.",
                "After verifying the encrypted copy, the app tries to remove exactly the source version it read. SAF and Windows keep the source, so the result is a copy. A changed source, or one that cannot be safely removed, is also kept."))
            Text(tr("Удаление не гарантирует стирания следов на флеш-памяти, в резервных копиях и других приложениях.",
                "Deletion does not guarantee erasing traces on flash storage, in backups, or in other apps."), color = MaterialTheme.colorScheme.error)
        } },
        confirmButton = { TextButton(enabled = enabled && files.isNotEmpty(), onClick = onConfirm) { Text(tr("Переместить", "Move")) } },
        dismissButton = { TextButton(onClick = onClose) { Text(tr("Отмена", "Cancel")) } })
}
