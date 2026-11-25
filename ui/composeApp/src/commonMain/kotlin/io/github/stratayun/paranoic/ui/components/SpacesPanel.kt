package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.Space
import io.github.stratayun.paranoic.ui.state.*

/** Places are navigation entries. Hiding one does not revoke its grant or delete files. */
@Composable
fun SpacesPanel(spaces: List<Space>, catalog: PlaceCatalog, connected: Boolean, busy: Boolean, onConnect: () -> Unit,
    onOpen: (Space) -> Unit, onDetach: (Space) -> Unit, modifier: Modifier = Modifier,
    onConnectServer: ((String, String, String) -> Unit)? = null) {
    var serverDialog by remember { mutableStateOf(false) }
    var confirm by remember { mutableStateOf<Space?>(null) }
    var renaming by remember { mutableStateOf<Space?>(null) }
    var name by remember { mutableStateOf("") }
    var info by remember { mutableStateOf<Space?>(null) }
    var showHidden by remember { mutableStateOf(false) }
    val editable = catalog.loaded && !catalog.busy && !catalog.failed && !busy
    fun appPrivate(space: Space) = space.localContainer && !space.detachable && !space.androidTree && !space.cloud
    val privateTitle = tr("Файлы приложения", "App files")
    fun title(space: Space) = catalog.title(space.id, if (appPrivate(space)) privateTitle else space.name)
    val hidden = spaces.filter { catalog.snapshot.find(it.id)?.hidden == true }
    val visible = spaces.filter { catalog.snapshot.find(it.id)?.hidden != true }
    val scheme = MaterialTheme.colorScheme
    fun symbol(space: Space) = when {
        appPrivate(space) -> AppSymbol.Vault
        space.cloud -> AppSymbol.Cloud
        space.androidTree -> AppSymbol.Folder
        else -> AppSymbol.Phone
    }
    val kindPrivate = tr("На этом устройстве · данные Paranoic", "On this device · Paranoic data")
    val kindCloud = tr("Облако или сервер", "Cloud or server")
    val kindTree = tr("Папка через системный доступ", "Folder with system access")
    val kindDevice = tr("Память устройства", "Device storage")
    val readOnly = tr(" · только чтение", " · read-only")
    fun kind(space: Space) = when {
        appPrivate(space) -> kindPrivate
        space.cloud -> kindCloud
        space.androidTree -> kindTree
        else -> kindDevice
    } + if (space.writable) "" else readOnly
    Column(modifier.padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        SectionLabel(tr("Места", "Places"))
        if (catalog.busy) {
            LinearProgressIndicator(Modifier.fillMaxWidth())
            Text(tr("Обновляем список мест…", "Updating places…"))
        }
        if (catalog.failed) {
            Text(tr("Не удалось прочитать или сохранить названия и видимость мест. Изменения приостановлены.",
                "Could not read or save place names and visibility. Changes are paused."), color = scheme.error)
            TextButton(enabled = !catalog.busy, onClick = catalog::load) { Text(tr("Перечитать", "Reload")) }
        }
        catalog.removedTitle?.let { removed ->
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Text(tr("Убрано из меню: ", "Hidden from menu: ") + displayName(removed), Modifier.weight(1f).announceChanges())
                TextButton(enabled = editable, onClick = catalog::undo) { Text(tr("Отменить", "Undo")) }
            }
        }
        if (visible.isEmpty()) Text(if (hidden.isEmpty())
            tr("Добавьте папку, чтобы начать работу с файлами.", "Add a folder to start browsing files.")
            else tr("Все подключённые места скрыты из меню.", "All connected places are hidden from the menu."),
            Modifier.padding(horizontal = 12.dp), color = scheme.onSurfaceVariant)
        GroupedList(visible) { space, position ->
            val index = visible.indexOf(space)
            var menu by remember(space.id) { mutableStateOf(false) }
            GroupedRow(displayName(title(space)), position, subtitle = kind(space), enabled = connected && !busy,
                leading = { ShapedIcon(symbol(space), ExpressiveShapes.gallery[index % ExpressiveShapes.gallery.size],
                    scheme.secondaryContainer, scheme.onSecondaryContainer) },
                trailing = {
                    Box {
                        IconButton(onClick = { menu = true },
                            modifier = Modifier.actionDescription(tr("Действия: ", "Actions: ") + displayName(title(space)))) { AppIcon(AppSymbol.More) }
                        DropdownMenu(menu, { menu = false }) {
                            DropdownMenuItem(text = { Text(tr("Переименовать в меню", "Rename in menu")) }, enabled = editable,
                                leadingIcon = { AppIcon(AppSymbol.Edit) }, onClick = { menu = false; renaming = space; name = title(space) })
                            DropdownMenuItem(text = { Text(tr("Скрыть из меню", "Hide from menu")) }, enabled = editable,
                                leadingIcon = { AppIcon(AppSymbol.Close) }, onClick = { menu = false; catalog.hide(space.id, title(space)) })
                            DropdownMenuItem(text = { Text(tr("Сведения", "Details")) },
                                leadingIcon = { AppIcon(AppSymbol.Info) }, onClick = { menu = false; info = space })
                            if (space.detachable) DropdownMenuItem(text = { Text(tr("Отозвать доступ", "Revoke access"), color = scheme.error) },
                                enabled = connected && !busy, leadingIcon = { AppIcon(AppSymbol.Trash) }, onClick = { menu = false; confirm = space })
                        }
                    }
                },
                onClick = { onOpen(space.copy(name = title(space))) })
        }
        SectionLabel(tr("Подключить", "Connect"))
        val connectRows = buildList {
            add(Triple(tr("Добавить папку", "Add folder"), AppSymbol.Add, onConnect))
            if (onConnectServer != null) add(Triple(tr("Сервер WebDAV", "WebDAV server"), AppSymbol.Server, { serverDialog = true }))
        }
        GroupedList(connectRows) { (label, icon, action), position ->
            GroupedRow(label, position, enabled = connected && !busy,
                leading = { ShapedIcon(icon, ExpressiveShapes.Circle, scheme.surfaceContainerHighest, scheme.onSurfaceVariant, size = 40.dp) },
                onClick = action)
        }
        if (hidden.isNotEmpty()) {
            TextButton(onClick = { showHidden = !showHidden }) {
                Text(if (showHidden) tr("Свернуть скрытые места", "Collapse hidden places")
                    else tr("Скрытые места (${hidden.size})", "Hidden places (${hidden.size})"))
            }
            if (showHidden) {
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 180.dp)) {
                    items(hidden, key = { it.id }) { space ->
                        TextButton(enabled = editable, onClick = { catalog.show(space.id) },
                            modifier = Modifier.actionDescription(tr("Вернуть в меню: ", "Show in menu: ") + displayName(title(space)))) {
                            Text(tr("Вернуть: ", "Show: ") + displayName(title(space)), maxLines = 2, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        }
    }
    if (serverDialog && onConnectServer != null)
        WebDavDialog({ serverDialog = false }) { url, user, token -> serverDialog = false; onConnectServer(url, user, token) }
    renaming?.let { space ->
        AlertDialog(onDismissRequest = { renaming = null; name = "" },
            title = { Text(tr("Название места", "Place name")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { if (it.length <= 120) name = it }, singleLine = true,
                    label = { Text(tr("Название в меню", "Menu name")) }, isError = !validPlaceTitle(name))
                Text(tr("Это подпись в Paranoic. Папка на диске не переименовывается.", "This is a label in Paranoic. The disk folder is not renamed."))
            } },
            confirmButton = { TextButton(enabled = editable && validPlaceTitle(name), onClick = {
                catalog.rename(space.id, name); renaming = null; name = ""
            }) { Text(tr("Сохранить", "Save")) } },
            dismissButton = { TextButton(onClick = { renaming = null; name = "" }) { Text(tr("Отмена", "Cancel")) } })
    }
    info?.let { space ->
        AlertDialog(onDismissRequest = { info = null }, title = { Text(displayName(title(space))) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (appPrivate(space)) tr(
                    "Это рабочая папка Paranoic в данных приложения. Служебные объекты и личная защищённая папка скрыты ядром. На Android эти данные удаляются вместе с приложением; на компьютере папка данных может остаться после удаления программы.",
                    "This is Paranoic's working folder in app data. Core hides service objects and the personal protected folder. Android removes this data with the app; desktop app data may remain after uninstall.")
                else tr("Paranoic работает только с выбранной папкой. «Убрать» скрывает её из этого меню, сохраняя доступ. «Отозвать доступ» отключает место, сохраняя файлы.",
                    "Paranoic accesses only the selected folder. Hide removes it from this menu and keeps access. Revoke access disconnects the place and keeps its files."))
                Text(if (space.writable) tr("Чтение и запись", "Read and write") else tr("Только чтение", "Read-only"))
                Text(tr("Обычные файлы здесь не шифруются автоматически. Для защиты создайте хранилище.", "Ordinary files here are not encrypted automatically. Create a vault to protect them."))
            } }, confirmButton = { TextButton(onClick = { info = null }) { Text(tr("Понятно", "Got it")) } })
    }
    confirm?.let { space ->
        AlertDialog(onDismissRequest = { confirm = null },
            title = { Text(tr("Отозвать доступ?", "Revoke access?")) },
            text = { Text(tr("Приложение отключит «${displayName(title(space))}». Файлы и карточки хранилищ сохранятся. Чтобы просто скрыть место из меню, используйте «Убрать».",
                "The app will disconnect \"${displayName(title(space))}\". Files and vault cards are kept. Use Hide to remove the place from the menu.")) },
            confirmButton = { TextButton(enabled = connected && !busy, onClick = { confirm = null; onDetach(space) }) { Text(tr("Отозвать доступ", "Revoke access")) } },
            dismissButton = { TextButton(onClick = { confirm = null }) { Text(tr("Отмена", "Cancel")) } })
    }
}
