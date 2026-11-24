package io.github.stratayun.paranoic.ui.screens.encryption

import io.github.stratayun.paranoic.ui.components.FaultNotice
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
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun VaultLauncher(core: CoreGateway, spaces: List<Space>, catalog: VaultCatalog, enabled: Boolean,
    format: FolderFormat, onFormat: (FolderFormat) -> Unit,
    folderReturn: VaultFolderReturn?, onFolderReturnConsumed: () -> Unit,
    backHandler: @Composable (Boolean, () -> Unit) -> Unit,
    onCreate: (FolderNode) -> Unit, onOpen: (FoundVault) -> Unit, onRelocate: (SavedVault, FoundVault) -> Unit,
    onRecovery: (FoundVault) -> Unit, onConnectFolder: (VaultFolderRequest) -> Unit) {
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
                Text(tr("Адрес изменится только после успешного открытия паролем. Совпадение формата само по себе не доказывает, что это прежнее хранилище. Файлы не перемещаются.",
                    "The location is updated only after a successful password unlock. Matching format alone does not prove this is the same vault. No files are moved."))
            } },
            confirmButton = { TextButton(enabled = interactive && catalog.loaded && !catalog.busy && !catalog.failed,
                onClick = { replacement = null; onRelocate(original, found) }) { Text(tr("Перейти к паролю", "Continue to password")) } },
            dismissButton = { TextButton(onClick = { replacement = null }) { Text(tr("Отмена", "Cancel")) } })
    } }
    LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            Text(tr("Хранилища", "Vaults"), Modifier.sectionHeading(), style = MaterialTheme.typography.headlineMedium)
            Text(tr("Выберите хранилище или создайте новое в подключённой папке.",
                "Choose a vault or create one in a connected folder."), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        item {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(enabled = interactive, onClick = { creating = false; relocating = null; replacement = null; probe = null; fault = null }) { Text(tr("Открыть существующее", "Open existing")) }
                OutlinedButton(enabled = interactive && catalog.loaded && !catalog.failed, onClick = { creating = true; createStep = 0; relocating = null; replacement = null; probe = null; fault = null }) { Text(tr("Создать новое", "Create new")) }
            }
        }
        item {
            if (catalog.busy || checkingSaved) {
                LinearProgressIndicator(Modifier.fillMaxWidth())
                Text(if (checkingSaved) tr("Проверяем сохранённое хранилище…", "Checking saved vault…") else tr("Сохраняем список хранилищ…", "Updating vault list…"))
            }
            if (catalog.failed) {
                Text(tr("Не удалось прочитать или сохранить список. Запись приостановлена, чтобы не затереть сохранённые карточки.",
                    "Could not read or save the list. Changes are paused to preserve saved cards."), color = MaterialTheme.colorScheme.error)
                Text(tr("Существующее хранилище можно найти и открыть только для чтения. Для создания и изменения настроек сначала перечитайте каталог.", "You can find and open an existing vault read-only. Reload the catalog before creating vaults or changing settings."))
                TextButton(enabled = !catalog.busy, onClick = catalog::load) { Text(tr("Перечитать список", "Reload list")) }
            }
            if (creating == null) fault?.let { FaultNotice(it) }
            if (missingSaved != null) Text(tr("Хранилище не найдено по сохранённому адресу. Оно могло быть перемещено или место недоступно. Нажмите «Найти».",
                "The saved vault was not found. It may have moved or its location is unavailable. Choose Find."))
            catalog.removed?.let { removed ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(tr("Карточка убрана: ", "Card removed: ") + displayName(removed.title), Modifier.weight(1f))
                    TextButton(enabled = !catalog.busy && !catalog.failed, onClick = catalog::undo) { Text(tr("Отменить", "Undo")) }
                }
            }
            if (catalog.loaded && catalog.snapshot.entries.isEmpty() && creating == null) Text(tr(
                "Пока нет сохранённых хранилищ. Откройте существующее или создайте новое — карточка появится после успешного открытия.",
                "No saved vaults yet. Open or create a vault; its card will appear after a successful open."))
        }
        if (creating == null) items(catalog.snapshot.entries, key = { "saved:" + it.location.storageId + ":" + it.location.path }) { saved ->
            val connected = spaces.any { it.id == saved.location.storageId }
            OutlinedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(displayName(saved.title), style = MaterialTheme.typography.titleLarge)
                    Text((if (saved.kind == VaultKind.Cryptomator) "Cryptomator" else "Paranoic 1.8") + " · " +
                        if (connected) tr("Заблокировано", "Locked") else tr("Место не подключено", "Location disconnected"),
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(displayName(spaces.firstOrNull { it.id == saved.location.storageId }?.name.orEmpty()) + " / " + displayName(saved.location.path),
                        style = MaterialTheme.typography.bodySmall)
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(enabled = interactive && connected, onClick = { savedToOpen = saved }) { Text(tr("Открыть", "Open")) }
                        TextButton(enabled = interactive, onClick = {
                            creating = false; relocating = saved; replacement = null; probe = null; fault = null; missingSaved = null
                            val space = spaces.firstOrNull { it.id == saved.location.storageId }
                            if (space != null) trail = listOf(FolderNode(space.root, space.name)) else connectFolder(saved)
                        }) { Text(tr("Найти", "Find")) }
                        TextButton(enabled = interactive && !catalog.busy && !catalog.failed, onClick = { renaming = saved; renameText = saved.title }) { Text(tr("Название", "Name")) }
                        TextButton(enabled = interactive && catalog.loaded && !catalog.failed, onClick = { configuring = saved }) { Text(tr("Настройки", "Settings")) }
                        TextButton(enabled = interactive && !catalog.busy && !catalog.failed, onClick = { catalog.remove(saved.location) }) { Text(tr("Убрать", "Remove")) }
                    }
                }
            }
        }
        if (creating != null) {
            item {
                attachmentFault?.let { error ->
                    FaultNotice(error)
                    if (error == CoreFault.OutcomeUnknown) Text(tr(
                        "Подключение не подтверждено. Проверьте список мест перед новой попыткой. Создание хранилища не запускалось.",
                        "Attachment was not confirmed. Check connected locations before trying again. Vault creation was not started."))
                }
            }
            item {
                TextButton(enabled = interactive, onClick = { creating = null; relocating = null; replacement = null; missingSaved = null }) {
                    Text(tr("К списку хранилищ", "Back to vault list"))
                }
            }
            item {
                Text(if (creating == true) when (createStep) {
                    0 -> tr("Шаг 1 из 6 · Формат", "Step 1 of 6 · Format")
                    1 -> tr("Шаг 2 из 6 · Место", "Step 2 of 6 · Location")
                    else -> tr("Шаг 3 из 6 · Папка", "Step 3 of 6 · Folder")
                }
                    else tr("Выберите папку и найдите хранилища до ввода пароля.", "Choose a folder and find vaults before entering a password."), style = MaterialTheme.typography.bodyMedium)
                if (creating == true && createStep == 0) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FolderFormat.entries.forEach { candidate ->
                            FilterChip(selected = candidate == format, enabled = interactive,
                                onClick = { onFormat(candidate) }, label = { Text(if (candidate == FolderFormat.Paranoic) "Paranoic 1.8" else "Cryptomator") })
                        }
                    }
                    TextButton(enabled = interactive, onClick = { details = !details }) {
                        Text(if (details) tr("Скрыть пояснение", "Hide explanation")
                            else tr("Подробнее о формате", "About the format"))
                    }
                    if (details) {
                        Text(if (format == FolderFormat.Paranoic)
                            tr("Экспериментальный формат Paranoic: внешний аудит ещё не пройден. Создаётся новая подпапка; существующее содержимое не заменяется.",
                                "Experimental Paranoic format: external audit is pending. A new subfolder is created; existing content is not replaced.")
                        else tr("Cryptomator: создание формата 8, открытие 7/8. Удаление целых последних блоков не обнаруживается форматом.",
                            "Cryptomator: create format 8, open 7/8. Removal of whole trailing chunks cannot be detected by the format."))
                    }
                }
            }
            if (creating == true && createStep == 0) {
                item { Button(enabled = interactive, onClick = { createStep = 1; trail = emptyList() }) { Text(tr("Далее: место", "Next: location")) } }
            } else {
            item {
                if (relocating != null) {
                    Text(tr("Ищем для карточки: ", "Finding a location for: ") + displayName(relocating!!.title))
                    TextButton(enabled = interactive, onClick = { relocating = null; replacement = null }) { Text(tr("Отменить поиск адреса", "Cancel relocation")) }
                }
                if (creating == true) TextButton(enabled = interactive, onClick = {
                    if (createStep == 2) { createStep = 1; trail = emptyList() } else createStep = 0
                }) { Text(tr("Назад", "Back")) }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    TextButton(enabled = interactive, onClick = { trail = emptyList(); if (creating == true) createStep = 1 }) { Text(tr("Все места", "All locations")) }
                    trail.forEachIndexed { index, node ->
                        TextButton(enabled = interactive, onClick = { trail = trail.take(index + 1) }) { Text(displayName(node.name)) }
                    }
                }
            }
            if (current == null) {
                items(spaces, key = { it.id }) { space ->
                    OutlinedCard(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(displayName(space.name), style = MaterialTheme.typography.titleMedium)
                            Text(if (space.cloud) tr("Подключённое облако или сервер", "Connected cloud or server") else tr("Подключённая папка", "Connected folder"), style = MaterialTheme.typography.bodySmall)
                            TextButton(enabled = interactive, onClick = { trail = listOf(FolderNode(space.root, space.name)); if (creating == true) createStep = 2 }) { Text(tr("Выбрать место", "Choose location")) }
                        }
                    }
                }
                item {
                    if (spaces.isEmpty()) Text(tr("Выберите папку через системное окно. Доступ ко всем файлам не требуется.", "Choose a folder using the system picker. All-files access is not required."))
                    TextButton(enabled = interactive, onClick = { connectFolder() }) { Text(tr("Подключить папку", "Connect folder")) }
                }
            } else {
                item {
                    Text(displayName(place?.name.orEmpty()) + " / " + displayName(current.location.path), style = MaterialTheme.typography.bodySmall)
                    if (reading || probing) {
                        LinearProgressIndicator(Modifier.fillMaxWidth())
                        Text(if (probing) tr("Ищем хранилища в папке и её подпапках…", "Looking for vaults in this folder and its direct subfolders…") else tr("Читаем папку…", "Reading folder…"))
                    }
                    fault?.let {
                        FaultNotice(it)
                        TextButton(enabled = interactive, onClick = { revision++ }) { Text(tr("Повторить чтение", "Read again")) }
                    }
                    if (creating == true) {
                        Button(enabled = interactive && catalog.loaded && !catalog.failed && loaded == location && place?.writable == true, onClick = { onCreate(current) }) { Text(tr("Создать в этой папке", "Create in this folder")) }
                        if (place?.writable != true) Text(tr("Это место доступно только для чтения.", "This location is read-only."))
                    } else {
                        Button(enabled = interactive && loaded == location, onClick = { fault = null; probe = null; probing = true; probeRevision++ }) { Text(tr("Найти хранилища", "Find vaults")) }
                    }
                }
                probe?.let { result ->
                    if (result.vaults.isEmpty()) item { Text(tr("Хранилище не найдено. Выберите другую папку или проверьте доступ.", "No vault found. Choose another folder or check access.")) }
                    if (result.unchecked > 0) item { Text(tr("Не удалось проверить объектов: ${result.unchecked}. Результат может быть неполным.", "Objects not checked: ${result.unchecked}. Results may be incomplete.")) }
                    items(result.vaults, key = { it.location.storageId + ":" + it.location.path }) { found ->
                        OutlinedCard(Modifier.fillMaxWidth()) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(displayName(found.name.ifEmpty { current.name }), style = MaterialTheme.typography.titleMedium)
                                Text(if (found.kind == VaultKind.Cryptomator) "Cryptomator" else "Paranoic 1.8")
                                Text(tr("Заблокировано", "Locked"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                                val original = relocating
                                if (original == null) {
                                    TextButton(enabled = interactive, onClick = { onOpen(found.copy(name = found.name.ifEmpty { current.name })) }) { Text(tr("Открыть", "Open")) }
                                } else {
                                    TextButton(enabled = interactive && found.kind == original.kind && catalog.loaded && !catalog.busy && !catalog.failed,
                                        onClick = { replacement = found }) { Text(tr("Выбрать для карточки", "Use for this card")) }
                                    if (found.kind != original.kind) Text(tr("Формат не совпадает с карточкой.", "The format differs from this card."))
                                }
                                if (relocating == null && found.kind == VaultKind.Paranoic) TextButton(enabled = interactive,
                                    onClick = { onRecovery(found.copy(name = found.name.ifEmpty { current.name })) }) { Text(tr("Ключ / забыли пароль…", "Recovery key / forgot password…")) }
                            }
                        }
                    }
                }
                items(folders, key = { it.location.storageId + ":" + it.location.path }) { folder ->
                    TextButton(enabled = interactive, onClick = { trail = trail + FolderNode(folder.location, folder.name) }) { Text(displayName(folder.name)) }
                }
                if (!reading && loaded == location && folders.isEmpty()) item { Text(tr("Вложенных папок нет.", "No subfolders."), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
            }
        }
    }
}
