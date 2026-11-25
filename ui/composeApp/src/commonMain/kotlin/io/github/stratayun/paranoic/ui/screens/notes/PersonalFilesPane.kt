package io.github.stratayun.paranoic.ui.screens.notes

import io.github.stratayun.paranoic.resources.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.screens.encryption.VaultMoveDialog
import io.github.stratayun.paranoic.ui.screens.encryption.VaultImportMoveDialog
import io.github.stratayun.paranoic.ui.screens.encryption.VaultFileWindow
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

/** Safe uses the authenticated notes session and ordinary vault calls. Never
 * reconstruct its physical path, put it in the vault catalog or expose a grant. */
@OptIn(ExperimentalLayoutApi::class, ExperimentalFoundationApi::class)
@Composable
internal fun PersonalFilesPane(session: AppSession, enabled: Boolean, onBusy: (Boolean) -> Unit, onLock: () -> Unit,
    incoming: IncomingDocuments = NoIncomingDocuments,
    secureImage: @Composable (ByteArray, Modifier) -> Unit = { _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) },
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit = { _, _, _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) }) {
    val core = session.core
    val scope = rememberCoroutineScope()
    val gateway = remember(core) { EncryptedFolderGateway(core, FolderFormat.Paranoic) }
    var requested by remember { mutableStateOf(false) }
    var handle by remember { mutableStateOf<String?>(null) }
    var path by remember { mutableStateOf<List<String>>(emptyList()) }
    var listing by remember { mutableStateOf<VaultListing?>(null) }
    var listedPath by remember { mutableStateOf<List<String>?>(null) }
    var revision by remember { mutableIntStateOf(0) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var dialog by remember { mutableStateOf<PersonalDialog?>(null) }
    var viewing by remember { mutableStateOf<Pair<VaultEntry, List<String>>?>(null) }
    var picking by remember { mutableStateOf(false) }
    var pickingMove by remember { mutableStateOf(false) }
    var importReport by remember { mutableStateOf<VaultMoveInReport?>(null) }
    var busy by remember { mutableStateOf(false) }
    var completed by remember { mutableStateOf(false) }
    val active = remember { mutableSetOf<Long>() }
    var nextOperation by remember { mutableLongStateOf(0) }
    var documentOperation by remember { mutableStateOf<Long?>(null) }
    fun begin(): Long {
        val id = ++nextOperation
        active += id; busy = true; onBusy(true)
        return id
    }
    fun finish(id: Long) { active -= id; busy = active.isNotEmpty(); onBusy(busy) }
    val writesReady = enabled && session.protectedWrite.phase == ProtectedWritePhase.Ready

    LaunchedEffect(core, requested) {
        if (!requested) return@LaunchedEffect
        val operation = begin()
        var opening = true
        var owned: String? = null
        try {
            fault = null
            val result = protectedWrite(session) { core.safeOpen().also { if (it is CoreResult.Value) owned = it.value } }
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> { handle = result.value; path = emptyList() }
                is CoreResult.Failure -> { fault = result.fault; requested = false }
            }
            finish(operation); opening = false
            if (handle != null) awaitCancellation()
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable; requested = false }
        finally {
            if (opening) finish(operation)
            owned?.let { value ->
                val closing = begin()
                try {
                    withContext(NonCancellable) {
                        val result = try { core.vaultLock(value) } catch (_: Exception) { CoreResult.Failure(CoreFault.Unavailable) }
                        if (result is CoreResult.Failure && result.fault !in setOf(CoreFault.InvalidName, CoreFault.Locked)) {
                            session.pause(); session.resume()
                        }
                    }
                } finally {
                    if (handle == value) { handle = null; listing = null; listedPath = null; path = emptyList() }
                    finish(closing)
                }
            }
        }
    }
    val opened = handle
    LaunchedEffect(core, opened, path, revision) {
        listing = null; listedPath = null
        if (opened == null) return@LaunchedEffect
        val operation = begin()
        try {
            fault = null
            val result = core.vaultList(opened, path)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> { listing = result.value; listedPath = path.toList() }
                is CoreResult.Failure -> { fault = result.fault; if (result.fault == CoreFault.Locked) requested = false }
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { finish(operation) }
    }
    val current = listing.takeIf { listedPath == path }
    val ready = opened != null && current != null && !busy && writesReady
    fun write(action: suspend (String, List<String>) -> CoreResult<Unit>) {
        val h = handle ?: return
        if (!ready) return
        val folder = path.toList()
        val operation = begin()
        fault = null; completed = false; importReport = null; dialog = null
        scope.launch {
            try {
                val result = protectedWrite(session) { action(h, folder) }
                currentCoroutineContext().ensureActive()
                when (result) {
                    is CoreResult.Value -> { completed = true; revision++; session.refreshFileTabs() }
                    is CoreResult.Failure -> {
                        fault = result.fault
                        if (result.fault.hasUnknownWriteOutcome()) { listing = null; listedPath = null }
                    }
                }
            } finally { finish(operation) }
        }
    }
    val scheme = MaterialTheme.colorScheme
    var addMenu by remember { mutableStateOf(false) }
    var sheetEntry by remember { mutableStateOf<VaultEntry?>(null) }
    var copyingMany by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        fault?.let { FaultNotice(it) }
        if (completed) Text(importReport?.let { if (it.sourceRemoved) tr("Перемещено в Сейф.", "Moved into the Safe.")
            else tr("Скопировано, исходник сохранён", "Copied; source kept") } ?: tr("Готово.", "Done."), color = scheme.primary)
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        if (opened == null) {
            Column(Modifier.fillMaxWidth().padding(top = 32.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                BrandIcon(Res.drawable.ic_safe, size = 116.dp)
                Text(tr("Личная папка", "Personal folder"), style = MaterialTheme.typography.titleLarge)
                Text(tr("Зашифрована ключом Сейфа, скрыта из «Файлов» и не синхронизируется.",
                    "Encrypted with the Safe key, hidden from Files and never synchronized."),
                    style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                Button(enabled = !busy && !requested && writesReady, onClick = { requested = true }, modifier = Modifier.height(52.dp)) {
                    AppIcon(AppSymbol.Forward); Spacer(Modifier.width(8.dp)); Text(tr("Открыть", "Open"))
                }
            }
        } else {
            Surface(shape = MaterialTheme.shapes.extraLarge, color = scheme.surfaceContainer) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(enabled = !busy && path.isNotEmpty(), onClick = { path = path.dropLast(1) },
                        modifier = Modifier.actionDescription(tr("На уровень выше", "Up one level"))) { AppIcon(AppSymbol.Up) }
                    Text(if (path.isEmpty()) tr("Личная папка", "Personal folder") else path.joinToString(" › ") { displayName(it) },
                        Modifier.weight(1f), style = MaterialTheme.typography.titleSmall, maxLines = 1)
                    IconButton(enabled = !busy, onClick = { revision++ }, modifier = Modifier.actionDescription(tr("Обновить", "Refresh"))) { AppIcon(AppSymbol.Refresh) }
                    Box {
                        FilledTonalIconButton(enabled = ready, onClick = { addMenu = true },
                            modifier = Modifier.actionDescription(tr("Добавить", "Add"))) { AppIcon(AppSymbol.Add) }
                        DropdownMenu(addMenu, { addMenu = false }) {
                            DropdownMenuItem(text = { Text(tr("Копировать файлы сюда…", "Copy files here…")) }, leadingIcon = { AppIcon(AppSymbol.Upload) },
                                onClick = { addMenu = false; copyingMany = true })
                            DropdownMenuItem(text = { Text(tr("Переместить файл в Сейф…", "Move a file into the Safe…")) }, leadingIcon = { AppIcon(AppSymbol.MoveTo) },
                                onClick = { addMenu = false; pickingMove = true; picking = true })
                            DropdownMenuItem(text = { Text(tr("Новая папка", "New folder")) }, leadingIcon = { AppIcon(AppSymbol.NewFolder) },
                                enabled = path.size < 64, onClick = { addMenu = false; dialog = PersonalDialog.Create })
                            DropdownMenuItem(text = { Text(tr("Текстовый файл / Markdown", "Text / Markdown file")) }, leadingIcon = { AppIcon(AppSymbol.NewFile) },
                                onClick = { addMenu = false; dialog = PersonalDialog.CreateText })
                        }
                    }
                }
            }
            current?.let { value ->
                IncomingFilesSection(incoming, ready, value.entries.map { it.name }) { id, name ->
                    write { h, p ->
                        val document = incoming.take(id) ?: return@write CoreResult.Failure(CoreFault.Changed)
                        try { core.vaultAddIncoming(h, p, name, document) } finally { document.discard() }
                    }
                }
                if (value.damaged > 0) Text(tr("Объектов с ошибкой проверки: ${value.damaged}. Они не показаны.",
                    "Objects that failed verification: ${value.damaged}. They are not displayed."), color = scheme.error)
                if (value.entries.isEmpty()) Column(Modifier.fillMaxWidth().padding(vertical = 24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    BrandIcon(Res.drawable.ic_folder, size = 100.dp)
                    Text(tr("Здесь пока пусто", "Nothing here yet"), style = MaterialTheme.typography.titleMedium)
                    FilledTonalButton(enabled = ready, onClick = { copyingMany = true }) {
                        AppIcon(AppSymbol.Upload); Spacer(Modifier.width(8.dp)); Text(tr("Добавить файлы", "Add files"))
                    }
                }
                value.entries.forEach { entry ->
                    Surface(Modifier.fillMaxWidth().clip(MaterialTheme.shapes.large).combinedClickable(enabled = !busy,
                        onLongClick = { sheetEntry = entry },
                        onClick = { if (entry.folder) { if (path.size < 64) path = path + entry.name } else viewing = entry to path.toList() }),
                        color = Color.Transparent) {
                        Row(Modifier.heightIn(min = 60.dp).padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            FileTypeIcon(entry.name, entry.folder, size = 44.dp)
                            Column(Modifier.weight(1f)) {
                                Text(displayName(entry.name), style = MaterialTheme.typography.titleMedium, maxLines = 2)
                                if (!entry.folder) Text(tr("${entry.size} байт", "${entry.size} bytes"), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
                            }
                            IconButton(onClick = { sheetEntry = entry }, modifier = Modifier.actionDescription(tr("Действия: ", "Actions: ") + displayName(entry.name))) { AppIcon(AppSymbol.More) }
                        }
                    }
                }
            }
            TextButton(enabled = !busy, onClick = onLock) { AppIcon(AppSymbol.Vault); Spacer(Modifier.width(8.dp)); Text(tr("Заблокировать Сейф", "Lock Safe")) }
        }
    }
    sheetEntry?.let { entry ->
        fun run(next: () -> Unit): () -> Unit = { sheetEntry = null; next() }
        FileActionSheet(entry.name, if (entry.folder) tr("Папка в Сейфе", "Folder in the Safe") else tr("${entry.size} байт", "${entry.size} bytes"),
            if (entry.folder) AppSymbol.Folder else AppSymbol.Document, listOf(
            SheetGroup(tr("Открыть", "Open"), listOfNotNull(
                SheetAction(if (entry.folder) tr("Открыть", "Open") else tr("Открыть внутри", "Open inside"), AppSymbol.Forward,
                    run = run { if (entry.folder) path = path + entry.name else viewing = entry to path.toList() }),
                if (!entry.folder) SheetAction(tr("Выгрузить в…", "Export to…"), AppSymbol.Download, ready, run = run { dialog = PersonalDialog.Export(entry) }) else null,
            )),
            SheetGroup(tr("Упорядочить", "Organize"), listOf(
                SheetAction(tr("Переименовать", "Rename"), AppSymbol.Edit, ready, run = run { dialog = PersonalDialog.Rename(entry) }),
                SheetAction(tr("В другую папку", "Move within Safe"), AppSymbol.MoveTo, ready, run = run { dialog = PersonalDialog.Move(entry) }),
                SheetAction(tr("Удалить", "Delete"), AppSymbol.Trash, ready, danger = true, run = run { dialog = PersonalDialog.Delete(entry) }),
            )),
        ), onDismiss = { sheetEntry = null })
    }
    if (copyingMany) CoreFilePickerDialog(core, session.files.spaces, tr("Копировать в личную папку", "Copy into the personal folder"), CorePickKind.Files,
        maxSelection = 32, onDismiss = { copyingMany = false }) { picked ->
        copyingMany = false
        val sources = picked.map { it.location }
        // Copies only; originals stay. An existing name fails that file, nothing is replaced.
        if (sources.isNotEmpty()) write { h, p ->
            var last: CoreResult<Unit> = CoreResult.Value(Unit)
            for (source in sources) { last = core.vaultAdd(h, p, source); if (last is CoreResult.Failure) break }
            last
        }
    }
    if (picking) CoreFilePickerDialog(core, session.files.spaces, tr("Файл для личной папки", "File for the personal folder"), CorePickKind.Files,
        onDismiss = { picking = false }, onPick = { picking = false; dialog = if (pickingMove) PersonalDialog.ImportMove(it.single()) else PersonalDialog.Import(it.single()) })
    when (val selected = dialog) {
        null -> Unit
        PersonalDialog.CreateText -> CreateTextDialog(current?.entries.orEmpty().map { it.name }.toSet(), onDismiss = { dialog = null }) { name ->
            write { h, p -> gateway.textCreate(h, p, name, "") }
        }
        PersonalDialog.Create -> PersonalNameDialog(tr("Новая папка", "New folder"), "", ready, current?.entries.orEmpty().map { it.name },
            onDismiss = { dialog = null }) { name -> write { h, p -> core.vaultMkdir(h, p, name) } }
        is PersonalDialog.Rename -> PersonalNameDialog(tr("Переименовать", "Rename"), selected.entry.name, ready,
            current?.entries.orEmpty().map { it.name }, onDismiss = { dialog = null }) { name ->
            if (selected.entry in current?.entries.orEmpty()) write { h, p -> core.vaultRename(h, p, selected.entry.name, name, selected.entry.folder) }
        }
        is PersonalDialog.Move -> if (opened != null) VaultMoveDialog(gateway, opened, path, selected.entry, onClose = { dialog = null }) { destination ->
            if (selected.entry in current?.entries.orEmpty()) write { h, p -> gateway.move(h, p, selected.entry, destination) }
        }
        is PersonalDialog.Import -> AlertDialog(onDismissRequest = { dialog = null }, title = { Text(tr("Копировать в личную папку?", "Copy into the personal folder?")) },
            text = { Text(displayName(selected.file.name) + "\n" + tr("Исходник останется на месте. Имя сохраняется, замены нет.", "The source stays in place. The name is kept; nothing is replaced.")) },
            confirmButton = { TextButton(enabled = ready && current?.entries.orEmpty().none { it.name == selected.file.name },
                onClick = { write { h, p -> core.vaultAdd(h, p, selected.file.location) } }) { Text(tr("Копировать", "Copy")) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text(tr("Отмена", "Cancel")) } })
        is PersonalDialog.ImportMove -> VaultImportMoveDialog(listOf(selected.file), ready, onClose = { dialog = null }) {
            val requestId = randomRequestId()
            write { h, p ->
                when (val result = gateway.moveIn(h, p, selected.file.location, requestId)) {
                    is CoreResult.Value -> { importReport = result.value; CoreResult.Value(Unit) }
                    is CoreResult.Failure -> result
                }
            }
        }
        is PersonalDialog.Delete -> AlertDialog(onDismissRequest = { dialog = null }, title = { Text(tr("Удалить из личной папки?", "Delete from the personal folder?")) },
            text = { Text(displayName(selected.entry.name) + "\n" + if (selected.entry.folder)
                tr("Удаляется только пустая папка. Отменить удаление нельзя.", "Only an empty folder can be deleted. Deletion cannot be undone.")
                else tr("Файл будет удалён из Сейфа. Открытые копии снаружи не затрагиваются.", "The file is deleted from the Safe. Plaintext copies outside are unaffected.")) },
            confirmButton = { TextButton(enabled = ready, onClick = {
                if (selected.entry in current?.entries.orEmpty()) write { h, p -> core.vaultDelete(h, p, selected.entry.name, selected.entry.folder) }
            }) { Text(tr("Удалить", "Delete")) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text(tr("Отмена", "Cancel")) } })
        is PersonalDialog.Export -> PersonalExportDialog(core, session.files.spaces, selected.entry, ready, onDismiss = { dialog = null }) { destination ->
            if (selected.entry in current?.entries.orEmpty()) write { h, p -> core.vaultExport(h, p, selected.entry.name, destination) }
        }
    }
    viewing?.let { selected -> opened?.let { h ->
        VaultFileWindow(session, gateway, h, selected.second, selected.first, enabled,
            secureImage, mediaPlayer, onBusy = { value ->
                if (value) { if (documentOperation == null) documentOperation = begin() }
                else { documentOperation?.let { finish(it) }; documentOperation = null }
            }, onClose = { viewing = null; revision++ })
    } }
}

private sealed interface PersonalDialog {
    data object Create : PersonalDialog
    data object CreateText : PersonalDialog
    data class Rename(val entry: VaultEntry) : PersonalDialog
    data class Move(val entry: VaultEntry) : PersonalDialog
    data class Delete(val entry: VaultEntry) : PersonalDialog
    data class Import(val file: FileItem) : PersonalDialog
    data class ImportMove(val file: FileItem) : PersonalDialog
    data class Export(val entry: VaultEntry) : PersonalDialog
}

@Composable
private fun PersonalNameDialog(title: String, initial: String, enabled: Boolean, occupied: List<String>,
    onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by remember { mutableStateOf(initial) }
    val ready = enabled && plausibleOperationName(name) && name !in occupied
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss, title = { Text(title) },
        text = { OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
            label = { Text(tr("Имя", "Name")) }, isError = name.isNotEmpty() && (!plausibleOperationName(name) || name in occupied)) },
        confirmButton = { TextButton(enabled = ready, onClick = { if (ready) onConfirm(name) }) { Text(tr("Сохранить", "Save")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
private fun PersonalExportDialog(core: CoreGateway, spaces: List<Space>, entry: VaultEntry, enabled: Boolean,
    onDismiss: () -> Unit, onExport: (Location) -> Unit) {
    var choosing by remember { mutableStateOf(false) }
    var parent by remember { mutableStateOf<FileItem?>(null) }
    var checked by remember { mutableStateOf(false) }
    val destination = parent?.location
    val ready = enabled && checked && destination != null && spaces.any { it.id == destination.storageId && it.writable }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss,
        title = { Text(tr("Создать открытую копию?", "Create a plaintext copy?")) },
        text = { Column(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(displayName(entry.name))
            Text(tr("В выбранной папке появится НЕЗАШИФРОВАННЫЙ файл. Он доступен другим приложениям и резервным копиям. Clear, блокировка и пароль под принуждением не удалят эту копию. Имя сохраняется; существующий файл не заменяется.",
                "The selected folder will contain an UNENCRYPTED file, accessible to other apps and backups. Clear, locking and a duress password will not remove it. The name is kept; an existing file is never replaced."), color = MaterialTheme.colorScheme.error)
            TextButton(onClick = { choosing = true }) { Text(tr("Выбрать папку…", "Choose folder…")) }
            parent?.let { Text(displayName(spaces.firstOrNull { s -> s.id == it.location.storageId }?.name.orEmpty()) + " / " + displayName(it.location.path), maxLines = 3) }
            LabeledCheckbox(tr("Понимаю: создаётся открытая копия вне Сейфа", "I understand: a plaintext copy is created outside the Safe"), checked, onChange = { checked = it })
        } },
        confirmButton = { TextButton(enabled = ready, onClick = { if (ready && destination != null) onExport(destination) }) { Text(tr("Расшифровать", "Decrypt")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
    if (choosing) CoreFilePickerDialog(core, spaces, tr("Назначение открытой копии", "Plaintext copy destination"), CorePickKind.Directory,
        writable = true, onDismiss = { choosing = false }, onPick = { parent = it.single(); choosing = false })
}
