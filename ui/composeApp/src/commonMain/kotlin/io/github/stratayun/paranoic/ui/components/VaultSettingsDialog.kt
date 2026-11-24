package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.VaultKind
import io.github.stratayun.paranoic.ui.state.*

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun VaultSettingsDialog(entry: SavedVault, defaultIdleSeconds: Int, onDismiss: () -> Unit,
    onSave: (String, Boolean, Int?) -> Unit, canSave: Boolean = true) {
    var title by remember(entry) { mutableStateOf(entry.title) }
    var readOnly by remember(entry) { mutableStateOf(entry.readOnly) }
    var idle by remember(entry) { mutableStateOf(entry.idleLockSeconds) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text(tr("Настройки хранилища", "Vault settings")) },
        text = { Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(title, { if (it.length <= 120) title = it }, singleLine = true,
                label = { Text(tr("Название карточки", "Card name")) }, isError = !validVaultTitle(title))
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline()
                .toggleable(readOnly, role = Role.Checkbox, onValueChange = { readOnly = it }), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Checkbox(readOnly, null); Text(tr("Открывать только для чтения", "Open read-only"), Modifier.weight(1f))
            }
            Text(tr("Изменения применяются при следующем открытии. Чтение и явное копирование из хранилища остаются доступны.",
                "Changes apply on the next unlock. Reading and explicit copying out remain available."))
            Text(tr("Автоблокировка бездействия", "Idle lock"))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                FilterChip(selected = idle == null, onClick = { idle = null }, label = { Text(tr("Общая: ${defaultIdleSeconds / 60} мин", "Default: ${defaultIdleSeconds / 60} min")) })
                VAULT_IDLE_LOCK_CHOICES.forEach { seconds -> FilterChip(selected = idle == seconds, onClick = { idle = seconds }, label = { Text(tr("${seconds / 60} мин", "${seconds / 60} min")) }) }
            }
            Text(tr("Таймаут очищает открытые ключи приложения, включая другие секреты. Уход в фон блокирует сразу при любом таймауте. Несохранённые правки могут быть потеряны.",
                "The timeout clears open app keys, including other secrets. Backgrounding locks immediately at any timeout. Unsaved edits may be lost."))
            Text(if (entry.kind == VaultKind.Cryptomator) tr("Cryptomator: создание формата 8, чтение 7/8. Удаление целых последних блоков формат не обнаруживает.",
                "Cryptomator: creates format 8, reads 7/8. The format cannot detect removal of whole trailing chunks.")
                else tr("Paranoic 1.8 — экспериментальный формат до завершения аудита.", "Paranoic 1.8 is experimental until the audit is complete."))
            Text(tr("Адрес: /", "Location: /") + displayName(entry.location.path))
            Text(tr("Смена пароля доступна после открытия с правом записи.", "Password changes are available after unlocking with write access."))
        } }, confirmButton = { TextButton(enabled = canSave && validVaultTitle(title), onClick = { onSave(title, readOnly, idle) }) { Text(tr("Сохранить", "Save")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
