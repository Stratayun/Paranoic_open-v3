package io.github.stratayun.paranoic.ui.screens.home

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.bridge.Space
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import org.jetbrains.compose.resources.DrawableResource

/**
 * Home (owner's mockup 27.11): the four main sections as large tiles, quick access and
 * connected storages. Quick access shows pinned folders, not recently opened files:
 * a list of recent names on the start screen would leak what the owner worked on.
 */
@Composable
fun HomeScreen(
    spaces: List<Space>,
    catalog: PlaceCatalog,
    onFiles: () -> Unit,
    onVaults: () -> Unit,
    onSafe: () -> Unit,
    onDiagnostics: () -> Unit,
    onOpenSpace: (Space) -> Unit,
    onOpenPath: (Space, String) -> Unit,
    onShowPlaces: () -> Unit,
    onPinFolder: () -> Unit,
) {
    val places = spaces.userPlaces()
    fun title(space: Space) = catalog.title(space.id, space.name)
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp)) {
        // Main sections, 2 × 2.
        val tiles = listOf(
            HomeTile(tr("Файлы", "Files"), tr("Память, папки и облака", "Storage, folders and clouds"), Res.drawable.ic_folder, onFiles),
            HomeTile(tr("Крипто-папки", "Crypto folders"), tr("Зашифрованные хранилища", "Encrypted vaults"), Res.drawable.ic_crypto_folder, onVaults),
            HomeTile(tr("Сейф", "Safe"), tr("Заметки и личная папка", "Notes and personal folder"), Res.drawable.ic_safe, onSafe),
            HomeTile(tr("Диагностика", "Diagnostics"), tr("Проверка системы и событий", "System checks and events"), Res.drawable.ic_diagnostics, onDiagnostics),
        )
        tiles.chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                row.forEach { tile -> SectionTile(tile, Modifier.weight(1f)) }
            }
        }

        // Quick access: pinned folders.
        HomeCard {
            CardHeader(tr("Быстрый доступ", "Quick access"), tr("Показать все", "Show all"), onShowPlaces)
            val bookmarks = catalog.snapshot.bookmarks
            if (bookmarks.isEmpty()) {
                CardRow(Res.drawable.ic_star, tr("Закрепите папку", "Pin a folder"),
                    tr("Закреплённые папки появятся здесь", "Pinned folders appear here"), onClick = onPinFolder)
            } else bookmarks.take(6).forEach { bookmark ->
                val space = places.firstOrNull { it.id == bookmark.storageId }
                CardRow(Res.drawable.ic_folder, displayName(bookmark.title),
                    space?.let { tr("Папка · ", "Folder · ") + displayName(title(it)) } ?: tr("Недоступно — место не подключено", "Unavailable — place not connected"),
                    enabled = space != null, onClick = { if (space != null) onOpenPath(space.copy(name = title(space)), bookmark.path) })
            }
        }

        // Storages and connections.
        HomeCard {
            CardHeader(tr("Хранилища и подключения", "Storage and connections"), null, null)
            val usageOf = LocalVolumeUsage.current
            if (places.isEmpty()) CardRow(Res.drawable.ic_folder_add, tr("Нет подключённых мест", "No connected places"),
                tr("Разрешите доступ к файлам или подключите сервер", "Allow file access or connect a server"), onClick = onShowPlaces)
            places.forEach { space ->
                val private = space.localContainer && !space.detachable && !space.cloud
                val icon = when {
                    space.cloud -> Res.drawable.ic_cloud
                    private -> Res.drawable.ic_shield
                    space.name.contains("SD", true) || space.name.contains("USB", true) -> Res.drawable.ic_external_disk
                    else -> Res.drawable.ic_tiles
                }
                val usage = if (space.cloud || private) null else usageOf(space.name)?.takeIf { it.totalBytes > 0 }
                StorageRow(icon, displayName(if (private) tr("Файлы приложения", "App files") else title(space)), usage,
                    status = when {
                        space.cloud -> tr("Подключено", "Connected")
                        private -> tr("Данные Paranoic на устройстве", "Paranoic data on this device")
                        usage == null -> tr("Память устройства", "Device storage")
                        else -> null
                    }, connected = space.cloud) { onOpenSpace(space.copy(name = title(space))) }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

private data class HomeTile(val title: String, val subtitle: String, val icon: DrawableResource, val onClick: () -> Unit)

@Composable
private fun SectionTile(tile: HomeTile, modifier: Modifier) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier.height(124.dp), shape = RoundedCornerShape(24.dp), color = scheme.surfaceContainer,
        border = BorderStroke(1.dp, scheme.outlineVariant)) {
        Row(Modifier.clickable(role = Role.Button, onClick = tile.onClick).padding(start = 8.dp, end = 10.dp),
            verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(tile.icon, size = 50.dp)
            Spacer(Modifier.width(6.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(tile.title, style = MaterialTheme.typography.titleSmall.copy(fontSize = 15.sp, lineHeight = 19.sp),
                    fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(tile.subtitle, style = MaterialTheme.typography.bodySmall.copy(fontSize = 12.sp, lineHeight = 15.sp),
                    color = scheme.onSurfaceVariant, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
internal fun HomeCard(content: @Composable ColumnScope.() -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(Modifier.fillMaxWidth(), shape = RoundedCornerShape(24.dp), color = scheme.surfaceContainer,
        border = BorderStroke(1.dp, scheme.outlineVariant)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp), content = content)
    }
}

@Composable
private fun CardHeader(title: String, action: String?, onAction: (() -> Unit)?) {
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium.copy(fontSize = 18.sp),
            fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (action != null && onAction != null) TextButton(onClick = onAction) {
            Text(action, color = MaterialTheme.colorScheme.onSurface, style = MaterialTheme.typography.labelLarge)
            Spacer(Modifier.width(4.dp))
            Icon(AppSymbol.Forward.vector(), null, Modifier.size(18.dp))
        }
    }
}

@Composable
private fun CardRow(icon: DrawableResource, title: String, subtitle: String, enabled: Boolean = true, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(Modifier.fillMaxWidth(), shape = RoundedCornerShape(18.dp), color = scheme.surfaceContainerHigh) {
        Row(Modifier.clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(icon, size = 44.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    color = if (enabled) scheme.onSurface else scheme.onSurfaceVariant)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1,
                    overflow = TextOverflow.Ellipsis)
            }
            Icon(AppSymbol.More.vector(), null, Modifier.size(22.dp), tint = scheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun StorageRow(icon: DrawableResource, title: String, usage: VolumeUsage?, status: String?, connected: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(Modifier.fillMaxWidth(), shape = RoundedCornerShape(18.dp), color = scheme.surfaceContainerHigh) {
        Row(Modifier.clickable(role = Role.Button, onClick = onClick).padding(horizontal = 12.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(icon, size = 48.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (usage != null) {
                    val used = 1f - usage.freeBytes.toFloat() / usage.totalBytes
                    Text(tr("Свободно: ", "Free: ") + gigabytes(usage.freeBytes) + tr(" из ", " of ") + gigabytes(usage.totalBytes),
                        style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        LinearProgressIndicator(progress = { used }, modifier = Modifier.weight(1f).height(8.dp),
                            color = if (used > 0.9f) scheme.error else scheme.tertiary, trackColor = scheme.surfaceContainerHighest,
                            strokeCap = androidx.compose.ui.graphics.StrokeCap.Round)
                        Spacer(Modifier.width(12.dp))
                        Text("${(used * 100).toInt()} %", style = MaterialTheme.typography.labelLarge)
                    }
                } else if (status != null) Row(verticalAlignment = Alignment.CenterVertically) {
                    if (connected) {
                        Surface(Modifier.size(18.dp), shape = CircleShape, color = scheme.tertiary) {}
                        Spacer(Modifier.width(8.dp))
                    }
                    Text(status, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                }
            }
            Spacer(Modifier.width(8.dp))
            Icon(AppSymbol.Forward.vector(), null, Modifier.size(22.dp), tint = scheme.onSurfaceVariant)
        }
    }
}

private fun gigabytes(bytes: Long): String {
    val tenths = bytes * 10 / (1024L * 1024 * 1024)
    return "${tenths / 10},${tenths % 10} ГБ"
}

/** "Modules": every part of the app as a tile (owner's foldable mockup), including ones with no screen yet. */
@Composable
fun ModulesScreen(
    onVaults: () -> Unit, onSafe: () -> Unit, onDiagnostics: () -> Unit, onTasks: () -> Unit, onSync: () -> Unit,
    onSettings: () -> Unit, onFiles: () -> Unit, onCleanup: (() -> Unit)?, onTrash: (() -> Unit)?, onSearch: (() -> Unit)?,
) {
    val modules = listOf(
        HomeTile(tr("Файлы", "Files"), tr("Память, папки и облака", "Storage, folders and clouds"), Res.drawable.ic_folder, onFiles),
        HomeTile(tr("Крипто-папки", "Crypto folders"), tr("Зашифрованные хранилища", "Encrypted vaults"), Res.drawable.ic_crypto_folder, onVaults),
        HomeTile(tr("Сейф", "Safe"), tr("Заметки и личная папка", "Notes and personal folder"), Res.drawable.ic_safe, onSafe),
        HomeTile(tr("Диагностика", "Diagnostics"), tr("Проверка системы и данных", "System and data checks"), Res.drawable.ic_diagnostics, onDiagnostics),
        HomeTile(tr("Задачи", "Tasks"), tr("Операции и их ход", "Operations and progress"), Res.drawable.ic_tasks, onTasks),
        HomeTile(tr("Синхронизация", "Sync"), tr("Резервные копии и сервер", "Backups and server"), Res.drawable.ic_sync, onSync),
        HomeTile(tr("Очистка", "Cleanup"), tr("Дубли и мусор", "Duplicates and junk"), Res.drawable.ic_cleanup, onCleanup ?: {}),
        HomeTile(tr("Корзина", "Trash"), tr("Удалённые файлы", "Deleted files"), Res.drawable.ic_trash_empty, onTrash ?: {}),
        HomeTile(tr("Поиск", "Search"), tr("По всем файлам", "All files"), Res.drawable.ic_search, onSearch ?: {}),
        HomeTile(tr("Настройки", "Settings"), tr("Параметры и безопасность", "Preferences and security"), Res.drawable.ic_settings, onSettings),
    )
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(tr("Модули Paranoic", "Paranoic modules"), Modifier.padding(start = 4.dp, top = 4.dp),
            style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
        modules.chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                row.forEach { tile -> ModuleTile(tile, Modifier.weight(1f)) }
                if (row.size == 1) Spacer(Modifier.weight(1f))
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun ModuleTile(tile: HomeTile, modifier: Modifier, iconSize: Dp = 52.dp) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier, shape = RoundedCornerShape(22.dp), color = scheme.surfaceContainer, border = BorderStroke(1.dp, scheme.outlineVariant)) {
        Column(Modifier.clickable(role = Role.Button, onClick = tile.onClick).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            BrandIcon(tile.icon, size = iconSize)
            Text(tile.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(tile.subtitle, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 2)
        }
    }
}
