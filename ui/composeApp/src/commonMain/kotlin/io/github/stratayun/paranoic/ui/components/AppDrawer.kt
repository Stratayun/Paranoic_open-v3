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
 * Lower part of the drawer (owner's layout, 25.11): sections Files /
 * Crypto folders / Safe, tools Sync, "Diagnostics and tasks" (tasks live
 * inside Diagnostics) and Settings, all rows in one style.
 */
@Composable
fun DrawerFooter(
    enabled: Boolean,
    runningTasks: Int,
    onSafe: () -> Unit,
    onVaults: () -> Unit,
    onSync: () -> Unit,
    onDiagnostics: () -> Unit,
    @Suppress("UNUSED_PARAMETER") onTasks: () -> Unit,
    onSettings: () -> Unit,
    modifier: Modifier = Modifier,
    onFiles: () -> Unit = {},
    current: Destination? = null,
    /** False when the internal-memory card above already opens Files (no duplicate entry). */
    showFiles: Boolean = true,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        SectionLabel(tr("Разделы", "Sections"))
        val sections = listOfNotNull(
            Triple(Destination.Files, Res.drawable.ic_tiles, onFiles).takeIf { showFiles },
            Triple(Destination.Encryption, Res.drawable.ic_crypto_folder, onVaults),
            Triple(Destination.Notes, Res.drawable.ic_safe, onSafe),
        )
        GroupedList(sections) { (target, icon, action), position ->
            GroupedRow(target.label(), position, enabled = enabled,
                subtitle = when (target) {
                    Destination.Files -> tr("Память, папки и облака", "Storage, folders and clouds")
                    Destination.Encryption -> tr("Cryptomator и Paranoic", "Cryptomator and Paranoic")
                    Destination.Notes -> tr("Заметки и личная папка", "Notes and personal folder")
                    else -> null
                },
                container = if (target == current) scheme.secondaryContainer else scheme.surfaceContainerHigh,
                leading = { BrandIcon(icon, size = 52.dp) }, onClick = action)
        }
        SectionLabel(tr("Инструменты", "Tools"))
        val rows = listOf(
            Triple(tr("Синхронизация и бэкап", "Sync and backup"), Res.drawable.ic_sync, onSync to tr("Свой сервер", "Own server")),
            Triple(tr("Диагностика и задачи", "Diagnostics and tasks"), Res.drawable.ic_diagnostics, onDiagnostics to
                (if (runningTasks > 0) tr("Выполняется задач: $runningTasks", "Running tasks: $runningTasks") else tr("Журнал, отчёт, задания", "Journal, report, jobs"))),
            // Owner (26.11): Settings is an ordinary row in the same style, not a separate button.
            Triple(tr("Настройки", "Settings"), Res.drawable.ic_settings, onSettings to tr("Тема, язык, безопасность", "Theme, language, security")),
        )
        GroupedList(rows) { (label, icon, pair), position ->
            GroupedRow(label, position, enabled = enabled, subtitle = pair.second,
                leading = { BrandIcon(icon, size = 52.dp) }, onClick = pair.first)
        }
    }
}
