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
fun DrawerHeader(title: String = "Paranoic", subtitle: String? = null, onClose: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 8.dp, top = 12.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
            if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        IconButton(onClick = onClose, modifier = Modifier.actionDescription(tr("Закрыть меню", "Close menu"))) { AppIcon(AppSymbol.Close) }
    }
}

/** One sidebar entry (owner's concept): small glass icon, label, highlighted when selected. */
@Composable
fun SidebarRow(label: String, icon: org.jetbrains.compose.resources.DrawableResource, selected: Boolean = false, enabled: Boolean = true,
    badge: String? = null, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(onClick = onClick, enabled = enabled, shape = RoundedCornerShape(16.dp),
        color = if (selected) scheme.secondaryContainer else androidx.compose.ui.graphics.Color.Transparent,
        modifier = Modifier.fillMaxWidth().actionDescription(label)) {
        Row(Modifier.padding(horizontal = 10.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(icon, size = 36.dp)
            Spacer(Modifier.width(12.dp))
            Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, maxLines = 2,
                color = if (enabled) scheme.onSurface else scheme.onSurfaceVariant)
            if (badge != null) Badge { Text(badge) }
        }
    }
}

/**
 * Lower part of the menu after the owner's concept (28.11): Modules (Home, Files, Crypto folders,
 * Safe, Diagnostics, Tasks), Tools (Sync and backup) and Settings at the end. The same block is
 * the phone drawer and the permanent sidebar on wide screens.
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
    /** False when the internal-memory card above already opens Files (no duplicate entry). */
    showFiles: Boolean = true,
    onHome: (() -> Unit)? = null,
    onModules: (() -> Unit)? = null,
) {
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        SectionLabel(tr("Модули", "Modules"))
        if (onHome != null) SidebarRow(tr("Главная", "Home"), Res.drawable.ic_home, current == Destination.Home, enabled, onClick = onHome)
        if (showFiles) SidebarRow(tr("Файлы", "Files"), Res.drawable.ic_folder, current == Destination.Files, enabled, onClick = onFiles)
        SidebarRow(tr("Крипто-папки", "Crypto folders"), Res.drawable.ic_crypto_folder, current == Destination.Encryption, enabled, onClick = onVaults)
        SidebarRow(tr("Сейф", "Safe"), Res.drawable.ic_safe, current == Destination.Notes, enabled, onClick = onSafe)
        SidebarRow(tr("Диагностика", "Diagnostics"), Res.drawable.ic_diagnostics, current == Destination.Diagnostics, enabled, onClick = onDiagnostics)
        SidebarRow(tr("Задачи", "Tasks"), Res.drawable.ic_tasks, current == Destination.Operations, enabled,
            badge = runningTasks.takeIf { it > 0 }?.toString(), onClick = onTasks)
        if (onModules != null) SidebarRow(tr("Все модули", "All modules"), Res.drawable.ic_modules, current == Destination.Modules, enabled, onClick = onModules)
        SectionLabel(tr("Инструменты", "Tools"))
        SidebarRow(tr("Синхронизация и бэкап", "Sync and backup"), Res.drawable.ic_sync, current == Destination.Sync, enabled, onClick = onSync)
        HorizontalDivider(Modifier.padding(vertical = 8.dp), color = MaterialTheme.colorScheme.outlineVariant)
        SidebarRow(tr("Настройки", "Settings"), Res.drawable.ic_settings, current == Destination.Settings, enabled, onClick = onSettings)
    }
}
