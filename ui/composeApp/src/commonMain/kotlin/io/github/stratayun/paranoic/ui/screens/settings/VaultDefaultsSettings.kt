package io.github.stratayun.paranoic.ui.screens.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun VaultDefaultsSettings(catalog: VaultCatalog) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(tr("Автоблокировка хранилищ", "Vault idle lock"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
        Text(tr("Общий таймаут можно переопределить в настройках карточки хранилища.", "Override this default in an individual vault card's settings."))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            VAULT_IDLE_LOCK_CHOICES.forEach { seconds -> FilterChip(selected = catalog.snapshot.defaultIdleLockSeconds == seconds,
                enabled = catalog.loaded && !catalog.busy && !catalog.failed, onClick = { catalog.defaultIdleLock(seconds) },
                label = { Text(tr("${seconds / 60} мин", "${seconds / 60} min")) }) }
        }
        Text(tr("Таймаут закрывает все ключи; уход в фон блокирует сразу.",
            "The timeout closes all keys; going to the background locks at once."))
        if (catalog.failed) {
            Text(tr("Сохранение приостановлено: каталог не прочитан или произошла ошибка записи.", "Saving is paused because the catalog could not be read or written."), color = MaterialTheme.colorScheme.error)
            TextButton(enabled = !catalog.busy, onClick = catalog::load) { Text(tr("Перечитать каталог", "Reload catalog")) }
        }
    }
}
