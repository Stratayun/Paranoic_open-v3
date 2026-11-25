package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.state.actionDescription
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
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        SectionLabel(tr("Инструменты", "Tools"))
        val rows = listOf(
            Triple(tr("Синхронизация и бэкап", "Sync and backup"), Res.drawable.ic_sync, onSync),
            Triple(tr("Диагностика", "Diagnostics"), Res.drawable.ic_stats, onDiagnostics),
        )
        GroupedList(rows) { (label, symbol, action), position ->
            GroupedRow(label, position, enabled = enabled,
                leading = { BrandIcon(symbol, size = 48.dp) },
                onClick = action)
        }
        Spacer(Modifier.height(12.dp))
        Spacer(Modifier.height(8.dp))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            val pill = Modifier.weight(1f).height(64.dp)
            FilledTonalButton(onTasks, pill, enabled = enabled, shape = RoundedCornerShape(topStart = 32.dp, bottomStart = 32.dp, topEnd = 8.dp, bottomEnd = 8.dp)) {
                AppIcon(AppSymbol.Tasks); Spacer(Modifier.width(8.dp))
                Text(if (runningTasks > 0) tr("Задачи · $runningTasks", "Tasks · $runningTasks") else tr("Задачи", "Tasks"))
            }
            FilledTonalButton(onSettings, pill, enabled = enabled, shape = RoundedCornerShape(topStart = 8.dp, bottomStart = 8.dp, topEnd = 32.dp, bottomEnd = 32.dp)) {
                AppIcon(AppSymbol.Settings); Spacer(Modifier.width(8.dp)); Text(tr("Настройки", "Settings"))
            }
        }
    }
}
