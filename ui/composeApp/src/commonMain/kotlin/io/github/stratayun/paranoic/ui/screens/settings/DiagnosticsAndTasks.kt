package io.github.stratayun.paranoic.ui.screens.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.BrandIcon
import io.github.stratayun.paranoic.ui.state.tr

/** Owner's layout (25.11): tasks live inside Diagnostics as a second tab. */
@Composable
fun DiagnosticsAndTasks(runningTasks: Int, startWithTasks: Boolean = false,
    diagnostics: @Composable () -> Unit, tasks: @Composable () -> Unit) {
    var tab by remember { mutableIntStateOf(if (startWithTasks) 1 else 0) }
    Column(Modifier.fillMaxSize()) {
        PrimaryTabRow(selectedTabIndex = tab, containerColor = MaterialTheme.colorScheme.surface) {
            Tab(selected = tab == 0, onClick = { tab = 0 }, text = { Text(tr("Диагностика", "Diagnostics")) },
                icon = { BrandIcon(Res.drawable.ic_diagnostics, size = 32.dp) })
            Tab(selected = tab == 1, onClick = { tab = 1 },
                text = { Text(if (runningTasks > 0) tr("Задачи · $runningTasks", "Tasks · $runningTasks") else tr("Задачи", "Tasks")) },
                icon = { BrandIcon(Res.drawable.ic_tasks, size = 32.dp) })
        }
        Box(Modifier.weight(1f).fillMaxWidth()) { if (tab == 0) diagnostics() else tasks() }
    }
}
