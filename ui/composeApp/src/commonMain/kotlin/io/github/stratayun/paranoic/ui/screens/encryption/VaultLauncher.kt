package io.github.stratayun.paranoic.ui.screens.encryption

import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.*
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** A vault is chosen independently of Files selection. No passwords are stored here.
 * Catalog browsing and probing never create data or open a secret session. */
@Composable
internal fun VaultLauncher(core: CoreGateway, spaces: List<Space>, catalog: VaultCatalog, enabled: Boolean,
    format: FolderFormat, onFormat: (FolderFormat) -> Unit,
    folderReturn: VaultFolderReturn?, onFolderReturnConsumed: () -> Unit,
    backHandler: @Composable (Boolean, () -> Unit) -> Unit,
    onCreate: (FolderNode) -> Unit, onOpen: (FoundVault) -> Unit, onRelocate: (SavedVault, FoundVault) -> Unit,
    onRecovery: (FoundVault) -> Unit, onConnectFolder: (VaultFolderRequest) -> Unit) {
    val spaces = spaces.userPlaces()
    var creating by remember { mutableStateOf<Boolean?>(null) }
    var trail by remember { mutableStateOf<List<FolderNode>>(emptyList()) }
    var folders by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var loaded by remember { mutableStateOf<Location?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var reading by remember { mutableStateOf(false) }
    var revision by remember { mutableIntStateOf(0) }
    var probing by remember { mutableStateOf(false) }
    var probeRevision by remember { mutableIntStateOf(0) }
    var probe by remember { mutableStateOf<VaultProbe?>(null) }
    var details by remember { mutableStateOf(false) }
    var createStep by remember { mutableIntStateOf(0) }
    var relocating by remember { mutableStateOf<SavedVault?>(null) }
    var replacement by remember { mutableStateOf<FoundVault?>(null) }
    var renaming by remember { mutableStateOf<SavedVault?>(null) }
    var configuring by remember { mutableStateOf<SavedVault?>(null) }
    var renameText by remember { mutableStateOf("") }
    var savedToOpen by remember { mutableStateOf<SavedVault?>(null) }
    var checkingSaved by remember { mutableStateOf(false) }
    var missingSaved by remember { mutableStateOf<Location?>(null) }
    var attachmentFault by remember { mutableStateOf<CoreFault?>(null) }
    LaunchedEffect(folderReturn) {
        val returned = folderReturn ?: return@LaunchedEffect
        val request = returned.request
        creating = request.creating
        trail = request.trail
        relocating = request.relocating
        createStep = if (trail.isEmpty()) 1 else 2
        replacement = null; probe = null; missingSaved = null
        attachmentFault = returned.fault
        onFormat(if (request.kind == VaultKind.Cryptomator) FolderFormat.Cryptomator else FolderFormat.Paranoic)
        onFolderReturnConsumed()
    }
    fun connectFolder(original: SavedVault? = relocating) {
        attachmentFault = null
        onConnectFolder(VaultFolderRequest(creating == true,
            if (format == FolderFormat.Cryptomator) VaultKind.Cryptomator else VaultKind.Paranoic,
            trail.toList(), original))
    }
    val current = trail.lastOrNull()
    val location = current?.location
    val place = spaces.firstOrNull { it.id == location?.storageId }
    LaunchedEffect(core, location, revision) {
        folders = emptyList(); loaded = null; fault = null; probe = null; probing = false
        reading = location != null
        if (location != null) {
            try {
                val result = core.list(location)
                currentCoroutineContext().ensureActive()
                when (result) {
                    is CoreResult.Value -> { folders = result.value.items.filter { it.kind == EntryKind.Directory }; loaded = location }
                    is CoreResult.Failure -> fault = result.fault
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { fault = CoreFault.Unavailable }
            finally { reading = false }
        }
    }
    LaunchedEffect(core, location, probeRevision) {
        if (probeRevision == 0 || !probing || location == null) return@LaunchedEffect
        try {
            val result = core.vaultProbe(location)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> probe = result.value
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { probing = false }
    }
    LaunchedEffect(core, savedToOpen) {
        val candidate = savedToOpen ?: return@LaunchedEffect
        checkingSaved = true; missingSaved = null; fault = null
        try {
            val result = core.vaultProbe(candidate.location)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    val found = result.value.vaults.singleOrNull { it.location == candidate.location && it.kind == candidate.kind }
                    if (found == null) missingSaved = candidate.location
                    else onOpen(found.copy(name = candidate.title))
                }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { checkingSaved = false; savedToOpen = null }
    }
    val interactive = enabled && !catalog.busy && !reading && !probing && !checkingSaved
    backHandler(creating != null) {
        if (interactive) {
            when {
                trail.size > 1 && (creating != true || createStep == 2) -> trail = trail.dropLast(1)
                creating == true && createStep == 2 -> { trail = emptyList(); createStep = 1 }
                creating == true && createStep == 1 -> createStep = 0
                creating != true && trail.isNotEmpty() -> trail = emptyList()
                else -> { creating = null; relocating = null; replacement = null }
            }
        }
    }
    configuring?.let { saved ->
        io.github.stratayun.paranoic.ui.components.VaultSettingsDialog(saved, catalog.snapshot.defaultIdleLockSeconds,
            onDismiss = { configuring = null }, canSave = interactive && catalog.loaded && !catalog.failed, onSave = { title, readOnly, idle ->
                catalog.settings(saved.location, title, readOnly, idle)
                configuring = null
            })
    }
    if (renaming != null) {
        val entry = renaming!!
        AlertDialog(onDismissRequest = { renaming = null; renameText = "" },
            title = { Text(tr("Название карточки", "Card name")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(renameText, { if (it.length <= 120) renameText = it }, singleLine = true,
                    label = { Text(tr("Название", "Name")) }, isError = !validVaultTitle(renameText))
                Text(tr("Имя папки и её содержимое не изменятся.", "The folder name and contents will not change."))
            } },
            confirmButton = { TextButton(enabled = validVaultTitle(renameText) && !catalog.busy,
                onClick = { catalog.rename(entry.location, renameText); renaming = null; renameText = "" }) { Text(tr("Сохранить", "Save")) } },
            dismissButton = { TextButton(onClick = { renaming = null; renameText = "" }) { Text(tr("Отмена", "Cancel")) } })
    }
    replacement?.let { found -> relocating?.let { original ->
        AlertDialog(onDismissRequest = { replacement = null },
            title = { Text(tr("Обновить адрес карточки?", "Update this card's location?")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(original.title), style = MaterialTheme.typography.titleMedium)
                Text(tr("Прежний адрес: ", "Previous location: ") + displayName(original.location.path))
                Text(tr("Новый адрес: ", "New location: ") + displayName(found.location.path))
                Text(tr("Адрес сменится только после открытия паролем. Файлы не перемещаются.",
                    "The location changes only after opening with the password. Files are not moved."))
            } },
            confirmButton = { TextButton(enabled = interactive && catalog.loaded && !catalog.busy && !catalog.failed,
                onClick = { replacement = null; onRelocate(original, found) }) { Text(tr("Перейти к паролю", "Continue to password")) } },
            dismissButton = { TextButton(onClick = { replacement = null }) { Text(tr("Отмена", "Cancel")) } })
    } }
    var addSheet by remember { mutableStateOf(false) }
    val scheme = MaterialTheme.colorScheme
    fun kindLabel(kind: VaultKind) = if (kind == VaultKind.Cryptomator) "Cryptomator" else "Paranoic"
    fun startOpen() { creating = false; relocating = null; replacement = null; probe = null; fault = null }
    fun startCreate() { creating = true; createStep = 0; relocating = null; replacement = null; probe = null; fault = null }
    val canCreate = interactive && catalog.loaded && !catalog.failed
    if (addSheet) VaultAddSheet(onDismiss = { addSheet = false },
        onOpen = { addSheet = false; startOpen() }, onCreate = { addSheet = false; startCreate() }, canOpen = interactive, canCreate = canCreate)
    Box(Modifier.fillMaxSize()) {
    LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp), contentPadding = PaddingValues(top = 8.dp, bottom = 112.dp)) {
        item {
            if (catalog.busy || checkingSaved) {
                LinearProgressIndicator(Modifier.fillMaxWidth())
                Text(if (checkingSaved) tr("Проверяем сохранённое хранилище…", "Checking saved vault…") else tr("Сохраняем список хранилищ…", "Updating vault list…"),
                    style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            if (catalog.failed) {
                Text(tr("Не удалось прочитать или сохранить список. Запись приостановлена, чтобы не затереть сохранённые карточки.",
                    "Could not read or save the list. Changes are paused to preserve saved cards."), color = scheme.error)
                Text(tr("Пока каталог не перечитан, хранилища открываются только для чтения.", "Until the catalog is reloaded, vaults open read-only."))
                TextButton(enabled = !catalog.busy, onClick = catalog::load) { Text(tr("Перечитать список", "Reload list")) }
            }
            if (creating == null) fault?.let { FaultNotice(it) }
            if (missingSaved != null) Text(tr("Хранилище не найдено: оно перемещено или место недоступно. Нажмите «Найти» в меню карточки.",
                "Vault not found: it moved or the place is unavailable. Use Find in the card menu."))
            catalog.removed?.let { removed ->
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(tr("Карточка убрана: ", "Card removed: ") + displayName(removed.title), Modifier.weight(1f))
                    TextButton(enabled = !catalog.busy && !catalog.failed, onClick = catalog::undo) { Text(tr("Отменить", "Undo")) }
                }
            }
        }
        if (creating == null && catalog.loaded && catalog.snapshot.entries.isEmpty()) item {
            Column(Modifier.fillMaxWidth().padding(top = 72.dp, start = 24.dp, end = 24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                BrandIcon(Res.drawable.ic_crypto_folder, size = 120.dp)
                Text(tr("Хранилищ пока нет", "No vaults yet"), style = MaterialTheme.typography.titleLarge)
                Text(tr("Подключите хранилище Cryptomator или Paranoic либо создайте новое.",
                    "Connect a Cryptomator or Paranoic vault, or create a new one."),
                    style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, textAlign = TextAlign.Center)
                FilledTonalButton(onClick = { addSheet = true }, enabled = interactive) { AppIcon(AppSymbol.Add); Spacer(Modifier.width(8.dp)); Text(tr("Добавить хранилище", "Add vault")) }
            }
        }
        if (creating == null && catalog.snapshot.entries.isNotEmpty()) item { SectionLabel(tr("Мои хранилища", "My vaults")) }
        if (creating == null) itemsIndexed(catalog.snapshot.entries, key = { _, it -> "saved:" + it.location.storageId + ":" + it.location.path }) { index, saved ->
            val connected = spaces.any { it.id == saved.location.storageId }
            var menu by remember(saved.location) { mutableStateOf(false) }
            GroupedRow(displayName(saved.title), groupPosition(index, catalog.snapshot.entries.size),
                subtitle = kindLabel(saved.kind) + " · " + (if (connected) tr("заблокировано", "locked") else tr("место не подключено", "location disconnected")) +
                    "\n" + displayName(spaces.firstOrNull { it.id == saved.location.storageId }?.name.orEmpty()) + " / " + displayName(saved.location.path),
                enabled = interactive && connected,
                leading = { BrandIcon(Res.drawable.ic_crypto_folder, size = 56.dp) },
                trailing = { Box {
                    IconButton(onClick = { menu = true }, modifier = Modifier.actionDescription(tr("Действия: ", "Actions: ") + displayName(saved.title))) { AppIcon(AppSymbol.More) }
                    DropdownMenu(menu, { menu = false }) {
                        DropdownMenuItem(text = { Text(tr("Найти по новому адресу", "Find at a new location")) }, leadingIcon = { AppIcon(AppSymbol.Search) }, enabled = interactive, onClick = {
                            menu = false; startOpen(); relocating = saved; missingSaved = null
                            val space = spaces.firstOrNull { it.id == saved.location.storageId }
                            if (space != null) trail = listOf(FolderNode(space.root, space.name)) else connectFolder(saved)
                        })
                        DropdownMenuItem(text = { Text(tr("Переименовать карточку", "Rename card")) }, leadingIcon = { AppIcon(AppSymbol.Edit) },
                            enabled = interactive && !catalog.failed, onClick = { menu = false; renaming = saved; renameText = saved.title })
                        DropdownMenuItem(text = { Text(tr("Настройки", "Settings")) }, leadingIcon = { AppIcon(AppSymbol.Settings) },
                            enabled = interactive && catalog.loaded && !catalog.failed, onClick = { menu = false; configuring = saved })
                        DropdownMenuItem(text = { Text(tr("Убрать из списка", "Remove from list"), color = scheme.error) }, leadingIcon = { AppIcon(AppSymbol.Close) },
                            enabled = interactive && !catalog.failed, onClick = { menu = false; catalog.remove(saved.location) })
                    }
                } },
                onClick = { savedToOpen = saved })
        }
        if (creating != null) {
            item {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(enabled = interactive, onClick = { creating = null; relocating = null; replacement = null; missingSaved = null },
                        modifier = Modifier.actionDescription(tr("К списку хранилищ", "Back to vault list"))) { AppIcon(AppSymbol.Close) }
                    Text(when {
                        relocating != null -> tr("Новый адрес хранилища", "New vault location")
                        creating == true -> tr("Создать хранилище", "Create vault")
                        else -> tr("Подключить хранилище", "Add existing vault")
                    }, Modifier.weight(1f).sectionHeading(), style = MaterialTheme.typography.titleLarge)
                }
                if (creating == true) {
                    val step = createStep.coerceIn(0, 2) + 1
                    LinearProgressIndicator(progress = { step / 6f }, modifier = Modifier.fillMaxWidth().padding(top = 4.dp))
                    Text(when (createStep) {
                        0 -> tr("Шаг 1 из 6 · Формат", "Step 1 of 6 · Format")
                        1 -> tr("Шаг 2 из 6 · Место", "Step 2 of 6 · Location")
                        else -> tr("Шаг 3 из 6 · Папка", "Step 3 of 6 · Folder")
                    }, Modifier.padding(top = 6.dp), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
                } else Text(tr("Выберите папку и найдите в ней хранилище. Пароль понадобится только на следующем шаге.",
                    "Pick a folder and find a vault in it. The password is asked on the next step."),
                    style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                attachmentFault?.let { error ->
                    FaultNotice(error)
                    if (error == CoreFault.OutcomeUnknown) Text(tr(
                        "Подключение не подтверждено. Проверьте список мест перед новой попыткой. Создание хранилища не запускалось.",
                        "Attachment was not confirmed. Check connected locations before trying again. Vault creation was not started."))
                }
                relocating?.let { original ->
                    Text(tr("Ищем для карточки: ", "Finding a location for: ") + displayName(original.title), Modifier.padding(top = 4.dp))
                }
            }
            if (creating == true && createStep == 0) {
                item { SectionLabel(tr("Формат", "Format")) }
                itemsIndexed(FolderFormat.entries) { index, candidate ->
                    val selected = candidate == format
                    Surface(Modifier.fillMaxWidth(), shape = groupShape(groupPosition(index, FolderFormat.entries.size)),
                        color = if (selected) scheme.secondaryContainer else scheme.surfaceContainerHigh,
                        border = if (selected) BorderStroke(2.dp, scheme.primary) else null) {
                        Row(Modifier.selectable(selected, enabled = interactive, role = Role.RadioButton, onClick = { onFormat(candidate) })
                            .padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            ShapedIcon(if (candidate == FolderFormat.Paranoic) AppSymbol.Security else AppSymbol.Vault,
                                if (candidate == FolderFormat.Paranoic) ExpressiveShapes.Cookie9 else ExpressiveShapes.Sunny,
                                scheme.primaryContainer, scheme.onPrimaryContainer)
                            Column(Modifier.weight(1f)) {
                                Text(if (candidate == FolderFormat.Paranoic) "Paranoic 1.8" else "Cryptomator", style = MaterialTheme.typography.titleMedium)
                                Text(if (candidate == FolderFormat.Paranoic)
                                    tr("Собственный формат. Экспериментальный: внешний аудит ещё не пройден.", "Own format. Experimental: external audit is pending.")
                                    else tr("Совместим с Cryptomator на ПК и телефоне. Создаётся формат 8, открываются 7 и 8.",
                                        "Compatible with Cryptomator on desktop and mobile. Creates format 8, opens 7 and 8."),
                                    style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                            }
                            RadioButton(selected = selected, onClick = null)
                        }
                    }
                }
                item {
                    TextButton(enabled = interactive, onClick = { details = !details }) {
                        Text(if (details) tr("Скрыть подробности", "Hide details") else tr("Подробнее о формате", "About the format"))
                    }
                    if (details) Text(if (format == FolderFormat.Paranoic)
                        tr("Создаётся новая подпапка; существующее содержимое не заменяется.", "A new subfolder is created; existing content is not replaced.")
                        else tr("Удаление целых последних блоков файла форматом Cryptomator не обнаруживается.", "Removal of whole trailing chunks cannot be detected by the Cryptomator format."),
                        style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                    Button(enabled = interactive, onClick = { createStep = 1; trail = emptyList() },
                        modifier = Modifier.fillMaxWidth().padding(top = 12.dp).height(56.dp)) { Text(tr("Далее: место", "Next: location")) }
                }
            } else {
                if (current == null) {
                    item { SectionLabel(tr("Где находится хранилище", "Where the vault is")) }
                    itemsIndexed(spaces, key = { _, it -> it.id }) { index, space ->
                        GroupedRow(displayName(space.name), groupPosition(index, spaces.size),
                            subtitle = (if (space.cloud) tr("Облако или сервер", "Cloud or server") else tr("Память устройства", "Device storage")) + if (space.writable) "" else tr(" · только чтение", " · read-only"),
                            enabled = interactive,
                            leading = { ShapedIcon(if (space.cloud) AppSymbol.Cloud else AppSymbol.Phone, ExpressiveShapes.gallery[index % ExpressiveShapes.gallery.size],
                                scheme.secondaryContainer, scheme.onSecondaryContainer) },
                            trailing = { AppIcon(AppSymbol.Forward) },
                            onClick = { trail = listOf(FolderNode(space.root, space.name)); if (creating == true) createStep = 2 })
                    }
                    item {
                        if (spaces.isEmpty()) Text(tr("Подключённых мест нет. Подключите место.", "No connected places. Connect a location."), color = scheme.onSurfaceVariant)
                        Spacer(Modifier.height(4.dp))
                        GroupedRow(tr("Подключить другое место", "Connect another location"), GroupPosition.Single, enabled = interactive,
                            leading = { BrandIcon(Res.drawable.ic_folder_add, size = 52.dp) },
                            onClick = { connectFolder() })
                        if (creating == true) TextButton(enabled = interactive, onClick = { createStep = 0 }) { Text(tr("← Назад к формату", "← Back to format")) }
                    }
                } else {
                    item {
                        FilesNavigation(trail) { index ->
                            if (interactive) trail = trail.take(index + 1)
                        }
                        Row(Modifier.padding(top = 4.dp)) {
                            TextButton(enabled = interactive, onClick = { trail = emptyList(); if (creating == true) createStep = 1 }) { Text(tr("← Все места", "← All places")) }
                        }
                        if (reading || probing) {
                            LinearProgressIndicator(Modifier.fillMaxWidth())
                            Text(if (probing) tr("Ищем хранилища в папке и её подпапках…", "Looking for vaults in this folder and its direct subfolders…") else tr("Читаем папку…", "Reading folder…"),
                                style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                        }
                        fault?.let {
                            FaultNotice(it)
                            TextButton(enabled = interactive, onClick = { revision++ }) { Text(tr("Повторить чтение", "Read again")) }
                        }
                        if (creating == true) {
                            Button(enabled = interactive && catalog.loaded && !catalog.failed && loaded == location && place?.writable == true, onClick = { onCreate(current) },
                                modifier = Modifier.fillMaxWidth().padding(top = 8.dp).height(56.dp)) {
                                AppIcon(AppSymbol.Add); Spacer(Modifier.width(8.dp)); Text(tr("Создать здесь", "Create here"))
                            }
                            if (place?.writable != true) Text(tr("Это место доступно только для чтения.", "This location is read-only."), color = scheme.error)
                        } else {
                            Button(enabled = interactive && loaded == location, onClick = { fault = null; probe = null; probing = true; probeRevision++ },
                                modifier = Modifier.fillMaxWidth().padding(top = 8.dp).height(56.dp)) {
                                AppIcon(AppSymbol.Search); Spacer(Modifier.width(8.dp)); Text(tr("Найти хранилища в этой папке", "Find vaults in this folder"))
                            }
                        }
                    }
                    probe?.let { result ->
                        item { SectionLabel(tr("Найдено", "Found")) }
                        if (result.vaults.isEmpty()) item { Text(tr("Хранилище не найдено. Выберите другую папку или проверьте доступ.", "No vault found. Choose another folder or check access."), color = scheme.onSurfaceVariant) }
                        if (result.unchecked > 0) item { Text(tr("Не удалось проверить объектов: ${result.unchecked}. Результат может быть неполным.", "Objects not checked: ${result.unchecked}. Results may be incomplete.")) }
                        itemsIndexed(result.vaults, key = { _, it -> "found:" + it.location.storageId + ":" + it.location.path }) { index, found ->
                            val name = found.name.ifEmpty { current.name }
                            val original = relocating
                            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                GroupedRow(displayName(name), if (original == null && found.kind == VaultKind.Paranoic) GroupPosition.First else GroupPosition.Single,
                                    subtitle = kindLabel(found.kind) + " · " + tr("заблокировано", "locked") +
                                        if (original != null && found.kind != original.kind) tr(" · формат не совпадает с карточкой", " · format differs from the card") else "",
                                    enabled = interactive && (original == null || (found.kind == original.kind && catalog.loaded && !catalog.busy && !catalog.failed)),
                                    container = scheme.secondaryContainer,
                                    leading = { BrandIcon(Res.drawable.ic_crypto_folder, size = 56.dp) },
                                    trailing = { Text(if (original == null) tr("Открыть", "Open") else tr("Выбрать", "Use"), color = scheme.primary, style = MaterialTheme.typography.labelLarge) },
                                    onClick = { if (original == null) onOpen(found.copy(name = name)) else replacement = found })
                                if (original == null && found.kind == VaultKind.Paranoic) GroupedRow(tr("Ключ восстановления / забыли пароль", "Recovery key / forgot password"),
                                    GroupPosition.Last, enabled = interactive,
                                    leading = { BrandIcon(Res.drawable.ic_lock, size = 52.dp) },
                                    onClick = { onRecovery(found.copy(name = name)) })
                            }
                        }
                    }
                    if (folders.isNotEmpty()) item { SectionLabel(tr("Папки", "Folders")) }
                    itemsIndexed(folders, key = { _, it -> "dir:" + it.location.storageId + ":" + it.location.path }) { index, folder ->
                        GroupedRow(displayName(folder.name), groupPosition(index, folders.size), enabled = interactive,
                            leading = { BrandIcon(Res.drawable.ic_folder, size = 52.dp) },
                            trailing = { AppIcon(AppSymbol.Forward) },
                            onClick = { trail = trail + FolderNode(folder.location, folder.name) })
                    }
                    if (!reading && loaded == location && folders.isEmpty()) item { Text(tr("Вложенных папок нет.", "No subfolders."), color = scheme.onSurfaceVariant) }
                }
            }
        }
    }
    if (creating == null) ExtendedFloatingActionButton(onClick = { addSheet = true }, expanded = catalog.snapshot.entries.isEmpty().not(),
        icon = { AppIcon(AppSymbol.Add) }, text = { Text(tr("Добавить", "Add")) },
        modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp))
    }
}

/** Reference: Vault Explorer "Добавить хранилище" sheet. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun VaultAddSheet(onDismiss: () -> Unit, onOpen: () -> Unit, onCreate: () -> Unit, canOpen: Boolean, canCreate: Boolean) {
    val scheme = MaterialTheme.colorScheme
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = scheme.surfaceContainerLow) {
        Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(tr("Добавить хранилище", "Add vault"), Modifier.padding(start = 8.dp, bottom = 16.dp), style = MaterialTheme.typography.titleLarge)
            GroupedRow(tr("Подключить существующее", "Add existing"), GroupPosition.First,
                subtitle = tr("Найти и разблокировать хранилище Cryptomator или Paranoic", "Find and unlock a Cryptomator or Paranoic vault"),
                enabled = canOpen, leading = { BrandIcon(Res.drawable.ic_crypto_folder, size = 56.dp) },
                trailing = { AppIcon(AppSymbol.Forward) }, onClick = onOpen)
            GroupedRow(tr("Создать новое", "Create new"), GroupPosition.Last,
                subtitle = tr("Новое зашифрованное хранилище в выбранной папке", "A new encrypted vault in a folder you choose"),
                enabled = canCreate, leading = { BrandIcon(Res.drawable.ic_folder_add, size = 56.dp) },
                trailing = { AppIcon(AppSymbol.Forward) }, onClick = onCreate)
        }
    }
}
