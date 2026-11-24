package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun IncomingFilesSection(incoming: IncomingDocuments, enabled: Boolean, occupied: List<String>,
    onImport: (id: String, name: String) -> Unit) {
    var selected by remember { mutableStateOf<String?>(null) }
    if (incoming.documents.isEmpty()) return
    Text(tr("Переданные файлы", "Shared files"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
    Text(tr("Временный доступ. Копия будет зашифрована прямо в текущую папку; исходник останется у отправителя. Повторного постоянного доступа приложение не получает.",
        "Temporary access. A copy is encrypted directly into the current folder; the sender keeps the source. The app does not obtain permanent access."),
        style = MaterialTheme.typography.bodySmall)
    incoming.documents.forEach { document ->
        Column {
            Text(displayName(document.name), maxLines = 2)
            Text(tr("${document.size} байт", "${document.size} bytes"), style = MaterialTheme.typography.bodySmall)
            Row {
                TextButton(enabled = enabled, onClick = { selected = document.id }) { Text(tr("Зашифровать сюда…", "Encrypt here…")) }
                TextButton(onClick = { incoming.discard(document.id) }) { Text(tr("Отклонить", "Dismiss")) }
            }
        }
    }
    val document = incoming.documents.firstOrNull { it.id == selected }
    if (document != null) key(document.id) {
        var name by remember { mutableStateOf(document.name) }
        var checked by remember { mutableStateOf(false) }
        val ready = enabled && plausibleOperationName(name) && name !in occupied && checked
        AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { selected = null },
            title = { Text(tr("Зашифровать переданный файл?", "Encrypt the shared file?")) },
            text = { Column(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(document.name))
                OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
                    label = { Text(tr("Имя в хранилище", "Name in the vault")) },
                    isError = name.isNotEmpty() && (!plausibleOperationName(name) || name in occupied))
                Text(tr("Существующий файл не заменяется. После попытки этот временный источник закрывается; для повторной отправки используйте «Поделиться» заново.",
                    "An existing file is never replaced. This temporary source closes after the attempt; share it again to resend."))
                LabeledCheckbox(tr("Подтверждаю копирование в текущую защищённую папку", "Copy into the current protected folder"), checked, onChange = { checked = it })
            } },
            confirmButton = { TextButton(enabled = ready, onClick = {
                if (ready) { selected = null; onImport(document.id, name) }
            }) { Text(tr("Зашифровать", "Encrypt")) } },
            dismissButton = { TextButton(onClick = { selected = null }) { Text(tr("Отмена", "Cancel")) } })
    }
}
