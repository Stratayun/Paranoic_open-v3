package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.navigation.Destination
import io.github.stratayun.paranoic.ui.state.actionDescription
import io.github.stratayun.paranoic.ui.state.label
import io.github.stratayun.paranoic.ui.state.tr

/** Drawer header: app name and a close button. */
@Composable
fun DrawerHeader(onClose: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 8.dp, top = 12.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Paranoic", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
        IconButton(onClick = onClose, modifier = Modifier.actionDescription(tr("Закрыть меню", "Close menu"))) { AppIcon(AppSymbol.Close) }
    }
}

/**
 * Lower part of the drawer, after the places (reference layout): system rows,
 * the accent "Safe" card, and a pair of pill buttons for tasks and settings.
 */
@Composable
fun DrawerFooter(
    enabled: Boolean,
    runningTasks: Int,
    onSafe: () -> Unit,
    onVaults: () -> Unit,
    onSync: () -> Unit,
    onDiagnostics: () -> Unit,
    onTasks: () -> Unit,
    onSettings: () -> Unit,
    modifier: Modifier = Modifier,
    onFiles: () -> Unit = {},
    current: Destination? = null,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        SectionLabel(tr("Разделы", "Sections"))
        val sections = listOf(
            Triple(Destination.Files, Res.drawable.ic_folder, onFiles),
            Triple(Destination.Encryption, Res.drawable.ic_vault, onVaults),
            Triple(Destination.Notes, Res.drawable.nav_safe, onSafe),
        )
        GroupedList(sections) { (target, icon, action), position ->
            GroupedRow(target.label(), position, enabled = enabled,
                subtitle = when (target) {
                    Destination.Encryption -> tr("Cryptomator и Paranoic", "Cryptomator and Paranoic")
                    Destination.Notes -> tr("Заметки и личная папка", "Notes and personal folder")
                    else -> null
                },
                container = if (target == current) scheme.secondaryContainer else scheme.surfaceContainerHigh,
                leading = { BrandIcon(icon, size = 48.dp) }, onClick = action)
        }
        SectionLabel(tr("Инструменты", "Tools"))
        val rows = listOf(
            Triple(tr("Синхронизация и бэкап", "Sync and backup"), Res.drawable.ic_sync, onSync),
            Triple(tr("Диагностика", "Diagnostics"), Res.drawable.ic_stats, onDiagnostics),
        )
        GroupedList(rows) { (label, symbol, action), position ->
            GroupedRow(label, position, enabled = enabled, leading = { BrandIcon(symbol, size = 48.dp) }, onClick = action)
        }
        Spacer(Modifier.height(12.dp))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            val pill = Modifier.weight(1f).height(64.dp)
            FilledTonalButton(onTasks, pill, enabled = enabled, shape = RoundedCornerShape(topStart = 32.dp, bottomStart = 32.dp, topEnd = 8.dp, bottomEnd = 8.dp)) {
                AppIcon(AppSymbol.Tasks); Spacer(Modifier.width(8.dp))
                Text(if (runningTasks > 0) tr("Задачи · $runningTasks", "Tasks · $runningTasks") else tr("Задачи", "Tasks"))
            }
            FilledTonalButton(onSettings, pill, enabled = enabled, shape = RoundedCornerShape(topStart = 8.dp, bottomStart = 8.dp, topEnd = 32.dp, bottomEnd = 32.dp),
                contentPadding = PaddingValues(start = 8.dp, end = 16.dp)) {
                BrandIcon(Res.drawable.nav_settings, size = 36.dp); Spacer(Modifier.width(6.dp)); Text(tr("Настройки", "Settings"))
            }
        }
    }
}
