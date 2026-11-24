package io.github.stratayun.paranoic.ui.screens.encryption

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.CreateTextDialog
import io.github.stratayun.paranoic.ui.components.IncomingFilesSection
import io.github.stratayun.paranoic.ui.components.PasswordTools
import io.github.stratayun.paranoic.ui.components.plausibleName
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay

private data class OpenVault(val handle: String, val name: String, val writable: Boolean, val root: Location)

/** Vault access uses its own location browser and marker probe. Files selection
 * is used only for explicit import/export actions after opening a vault.
 * Quick Lock discards access flow state and any resident UI credentials. */
@Composable
fun EncryptionScreen(session: AppSession, onBrowseFiles: () -> Unit) {
    EncryptionScreen(session, remember { DocumentExitGuard() }, onBrowseFiles = onBrowseFiles)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun EncryptionScreen(session: AppSession, exitGuard: DocumentExitGuard,
    folderReturn: VaultFolderReturn? = null, onFolderReturnConsumed: () -> Unit = {},
    onConnectFolder: ((VaultFolderRequest) -> Unit)? = null,
    backHandler: @Composable (Boolean, () -> Unit) -> Unit = { _, _ -> }, incoming: IncomingDocuments = NoIncomingDocuments,
    secureImage: @Composable (ByteArray, Modifier) -> Unit = { _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) },
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit = { _, _, _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) },
    onBrowseFiles: () -> Unit) {
    val core = session.core
    val scope = rememberCoroutineScope()
    var format by remember { mutableStateOf(FolderFormat.Paranoic) }
    val gateway = remember(core, format) { EncryptedFolderGateway(core, format) }
    var details by remember { mutableStateOf(false) }
    var readRevision by remember { mutableLongStateOf(0) }
    var vault by remember { mutableStateOf<OpenVault?>(null) }
    var path by remember { mutableStateOf<List<String>>(emptyList()) }
    var listing by remember { mutableStateOf<VaultListing?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var uncertain by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var reading by remember { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<Dialog?>(null) }
    var viewing by remember { mutableStateOf<Pair<VaultEntry, List<String>>?>(null) }
    var recoveryText by remember { mutableStateOf<String?>(null) }
    var recoveryAccess by remember { mutableStateOf(false) }
    var completed by remember { mutableStateOf<FolderWriteNotice?>(null) }
    var importReports by remember { mutableStateOf<List<VaultMoveInReport>>(emptyList()) }
    var interaction by remember(vault?.handle) { mutableLongStateOf(0L) }
    val idleSeconds = vault?.let { opened -> session.vaultCatalog.snapshot.entries.firstOrNull { it.location == opened.root }?.idleLockSeconds }
        ?: session.vaultCatalog.snapshot.defaultIdleLockSeconds
    LaunchedEffect(vault?.handle, idleSeconds, interaction) {
        if (vault != null) {
            delay(idleSeconds * 1000L)
            // Reuse Quick Lock: invalidate late secret calls and discard all resident credentials.
            session.pause(); session.resume()
        }
    }
    fun setBusy(value: Boolean) { busy = value; exitGuard.busy = value }
    DisposableEffect(exitGuard) {
        onDispose { exitGuard.busy = false; recoveryText = null; dialog = null }
    }

    DisposableEffect(vault, gateway) {
        val opened = vault
        onDispose { if (opened != null) scope.launch(NonCancellable) { safe { gateway.lock(opened.handle) } } }
    }
    LaunchedEffect(session.secretsEpoch) {
        ++readRevision; vault = null; path = emptyList(); listing = null; recoveryText = null; dialog = null; viewing = null
    }

    val files = session.files
    val selectedFiles = files.entries.filter { it.location in files.selected && it.kind == EntryKind.File }
    val here = files.trail.lastOrNull()

    fun reload(v: OpenVault, p: List<String>) {
        val request = ++readRevision
        reading = true
        listing = null
        scope.launch {
            val r = safe { gateway.list(v.handle, p) }
            if (request != readRevision || vault?.handle != v.handle) return@launch
            reading = false
            when (r) {
                is CoreResult.Value -> { listing = r.value; path = p }
                is CoreResult.Failure -> { fault = r.fault; if (r.fault == CoreFault.Locked) { vault = null; listing = null; recoveryText = null; dialog = null } }
            }
        }
    }
    backHandler(vault != null) {
        if (!busy) {
            val opened = vault
            if (opened != null && path.isNotEmpty()) reload(opened, path.dropLast(1))
            else { session.pause(); session.resume() }
        }
    }
    /** One write at a time; an unknown outcome is reported, never retried by itself. */
    fun act(notice: FolderWriteNotice? = null, block: suspend () -> CoreResult<*>) {
        if (busy || uncertain) return
        if (session.protectedWrite.phase != ProtectedWritePhase.Ready) { fault = CoreFault.OutcomeUnknown; return }
        setBusy(true); fault = null; recoveryAccess = false; completed = null; importReports = emptyList()
        scope.launch {
            try {
                val r = safe { block() }
                if (r is CoreResult.Failure) { fault = r.fault; uncertain = unknownOutcome(r.fault) }
                else completed = notice
                vault?.let { reload(it, path) }
            } finally { setBusy(false) }
        }
    }

    CompositionLocalProvider(LocalSecretActivity provides { if (vault != null) interaction++ }) {
    Column(Modifier.fillMaxSize().padding(16.dp).trackSecretActivity(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Column(Modifier.heightIn(max = 180.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        completed?.let { result -> Text(when (result) {
            FolderWriteNotice.Password -> tr("Пароль изменён. При следующем открытии используйте новый пароль.", "Password changed. Use the new password on the next open.")
            FolderWriteNotice.Move -> tr("Объект перенесён. Имя сохранено.", "Entry moved. Its name is unchanged.")
            FolderWriteNotice.Replace -> tr("Содержимое заменено. Исходный файл сохранён.", "Contents replaced. The source file is kept.")
        }) }
        if (importReports.isNotEmpty()) {
            val removed = importReports.count { it.sourceRemoved }
            val kept = importReports.size - removed
            if (removed > 0) Text(tr("Перемещено в хранилище: $removed", "Moved into the vault: $removed"))
            if (kept > 0) Text(tr("Скопировано, исходник сохранён", "Copied; source kept") + " ($kept)")
        }
        fault?.let { FaultNotice(it, message = if (recoveryAccess && it in setOf(CoreFault.InvalidName, CoreFault.WrongPassword))
            tr("Ключ восстановления не подошёл, имеет неверный формат или данные повреждены.",
                "The recovery key does not match, has an invalid format, or the data is damaged.") else it.label()) }
        if (uncertain) {
            Text(tr("Результат неизвестен. Проверьте задания и папку. Закройте хранилище и откройте заново; после смены пароля может потребоваться новый пароль. Новые записи заблокированы, автоматического повтора нет.",
                "Outcome unknown. Inspect jobs and the folder. Lock and reopen the vault; a password change may require the new password. New writes are blocked; there is no automatic retry."), color = MaterialTheme.colorScheme.error)
            if (vault == null) TextButton(enabled = !busy, onClick = { uncertain = false; fault = null }) {
                Text(tr("Проверил(а) задания и папку", "I inspected jobs and the folder"))
            }
        }
        if (format == FolderFormat.Cryptomator) {
            TextButton(onClick = { details = !details }) { Text(tr("Особенности формата", "Format details")) }
            if (details) Text(tr("Cryptomator не обнаруживает удаление целых последних блоков файла. Размер в списке — размер шифротекста, не точная длина исходника. Сбой изменения может потребовать ручного разбора в задачах.",
                "Cryptomator cannot detect removal of whole trailing file blocks. Listed sizes are ciphertext sizes, not exact plaintext lengths. An interrupted change may require manual review in Tasks."))
        }
        }
        val open = vault
        if (!core.connected) { FaultNotice(CoreFault.Disconnected); return@Column }
        if (open == null) {
            if (busy) {
                LinearProgressIndicator(Modifier.fillMaxWidth())
                Text(tr("Обрабатываем запрос хранилища. Это может занять время…", "Processing the vault request. This may take a while…"))
            }
            key(session.secretsEpoch) {
                Box(Modifier.weight(1f)) {
                    VaultLauncher(core, files.spaces, session.vaultCatalog, !busy && !uncertain && !session.writing, format,
                        folderReturn = folderReturn, onFolderReturnConsumed = onFolderReturnConsumed,
                        backHandler = backHandler,
                        onFormat = { format = it; fault = null; recoveryAccess = false },
                        onCreate = { node -> dialog = Dialog.Access(true, node) },
                        onOpen = { found ->
                            format = if (found.kind == VaultKind.Cryptomator) FolderFormat.Cryptomator else FolderFormat.Paranoic
                            fault = null; recoveryAccess = false
                            dialog = Dialog.Access(false, FolderNode(found.location, found.name))
                        },
                        onRelocate = { original, found ->
                            format = if (found.kind == VaultKind.Cryptomator) FolderFormat.Cryptomator else FolderFormat.Paranoic
                            fault = null; recoveryAccess = false
                            dialog = Dialog.Access(false, FolderNode(found.location, original.title), original)
                        },
                        onRecovery = { found ->
                            format = FolderFormat.Paranoic
                            dialog = Dialog.RecoveryOpen(FolderNode(found.location, found.name))
                        }, onConnectFolder = { request ->
                            if (onConnectFolder != null) onConnectFolder(request) else onBrowseFiles()
                        })
                }
            }
        } else {
            val l = listing
            if (session.vaultCatalog.failed) Text(tr("Хранилище открыто, но сохранить карточку не удалось. Её прежние данные сохранены; перечитайте каталог после блокировки.",
                "The vault is open, but its card could not be saved. Previous details are retained; reload the catalog after locking."), color = MaterialTheme.colorScheme.error)
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            item {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedCard(Modifier.fillMaxWidth()) { Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(open.name) + path.joinToString("") { " / " + displayName(it) }, Modifier.weight(1f),
                    style = MaterialTheme.typography.titleMedium)
                OutlinedButton(enabled = !busy, onClick = {
                    session.pause(); session.resume()
                }) { Text(tr("Заблокировать", "Lock")) }
            }
            Text((if (format == FolderFormat.Cryptomator) "Cryptomator" else "Paranoic 1.8") + " · " + tr("Открыто", "Unlocked"))
            Text(tr("Место: /", "Location: /") + displayName(open.root.path), style = MaterialTheme.typography.bodySmall)
            Text(tr("Автоблокировка: ${idleSeconds / 60} мин бездействия", "Idle lock: ${idleSeconds / 60} min"), style = MaterialTheme.typography.bodySmall)
            if (!open.writable) Text(tr("Хранилище доступно только для чтения.", "This vault is read-only."))
            Text(tr("Настройки карточки доступны после блокировки.", "Card settings are available after locking."), style = MaterialTheme.typography.bodySmall)
            } }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(enabled = !busy && !reading && !uncertain && open.writable,
                    onClick = { dialog = Dialog.ChangePassword }) { Text(tr("Сменить пароль…", "Change password…")) }
                if (format == FolderFormat.Paranoic) TextButton(enabled = !busy && !reading && !uncertain && open.writable,
                    onClick = { dialog = Dialog.RecoverySetup }) { Text(tr("Создать ключ восстановления…", "Create recovery key…")) }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (path.isNotEmpty()) TextButton(enabled = !busy && !reading, onClick = { reload(open, path.dropLast(1)) }) { Text(tr("↑ Вверх", "↑ Up")) }
                TextButton(enabled = !busy && !reading && !uncertain && open.writable, onClick = { dialog = Dialog.Mkdir }) { Text(tr("+ Папка", "+ Folder")) }
                TextButton(enabled = !busy && !reading && !uncertain && open.writable,
                    onClick = { dialog = Dialog.CreateText }) { Text(tr("+ Текст", "+ Text")) }
                TextButton(enabled = !busy && !reading && !uncertain && open.writable && selectedFiles.isNotEmpty(), onClick = {
                    val sources = selectedFiles.map { it.location }
                    act {
                        var last: CoreResult<Unit> = CoreResult.Value(Unit)
                        for (s in sources) { last = gateway.add(open.handle, path, s); if (last is CoreResult.Failure) break }
                        last
                    }
                }) { Text(tr("Зашифровать выделенные (${selectedFiles.size})", "Encrypt selected (${selectedFiles.size})")) }
                if (format == FolderFormat.Paranoic) TextButton(enabled = !busy && !reading && !uncertain && open.writable && selectedFiles.isNotEmpty(),
                    onClick = { dialog = Dialog.ImportMove(selectedFiles.toList(), path.toList()) }) {
                    Text(tr("Переместить в хранилище (${selectedFiles.size})…", "Move into vault (${selectedFiles.size})…"))
                }
            }
            if (selectedFiles.isEmpty()) Text(tr("Чтобы добавить файлы, выделите их в файловом менеджере. «Зашифровать» сохраняет исходники; «Переместить» требует отдельного подтверждения.",
                "To add files, select them in the file manager. Encrypt keeps the sources; Move requires separate confirmation."), style = MaterialTheme.typography.bodySmall)
            if (format == FolderFormat.Paranoic) Text(tr("Для замены содержимого выделите один исходный файл в разделе файлов, затем откройте папку 1.8. Имя заменяемого файла сохранится.",
                "To replace content, select one source file in Files, then open the 1.8 vault. The vault file keeps its name."), style = MaterialTheme.typography.bodySmall)
            if (busy || reading) LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            }
            if (l != null) {
                if (format == FolderFormat.Paranoic) item {
                    IncomingFilesSection(incoming, !busy && !reading && !uncertain && open.writable, l.entries.map { it.name }) { id, name ->
                        act {
                            val document = incoming.take(id) ?: return@act CoreResult.Failure(CoreFault.Changed)
                            try { gateway.addIncoming(open.handle, path.toList(), name, document) } finally { document.discard() }
                        }
                    }
                }
                if (format == FolderFormat.Paranoic && l.damaged > 0) item { Text(tr("Повреждённых или подменённых записей: ${l.damaged} (не показаны).",
                    "Damaged or tampered entries: ${l.damaged} (not shown)."), color = MaterialTheme.colorScheme.error) }
                if (format == FolderFormat.Cryptomator && l.damaged > 0) item { Text(tr("Не показано неподдерживаемых объектов: ${l.damaged}", "Unsupported objects omitted: ${l.damaged}")) }
                if (l.entries.isEmpty()) item { Text(tr("Папка пуста.", "The folder is empty.")) }
                    items(l.entries, key = { (if (it.folder) "d:" else "f:") + it.name }) { e ->
                        Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).then(if (e.folder) Modifier.keyboardFocusOutline().clickable(enabled = !busy && !reading,
                            role = Role.Button, onClickLabel = tr("Открыть папку", "Open folder")) { reload(open, path + e.name) } else Modifier),
                            verticalAlignment = Alignment.CenterVertically) {
                            Text((if (e.folder) "📁 " else "📄 ") + displayName(e.name), Modifier.weight(1f))
                            if (!e.folder) Text((if (format == FolderFormat.Cryptomator) tr("Шифротекст: ", "Ciphertext: ") else "") + formatSize(e.size), style = MaterialTheme.typography.bodySmall)
                        }
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (!e.folder) {
                                TextButton(enabled = !busy && !reading,
                                    onClick = { viewing = e to path.toList() }) { Text(tr("Открыть внутри", "Open inside")) }
                                TextButton(enabled = !busy && !reading && !uncertain && here != null && files.writable, onClick = {
                                    here?.let { to -> dialog = Dialog.Export(e, to) }
                                }) { Text(tr("Расшифровать", "Decrypt")) }
                            }
                            if (format == FolderFormat.Paranoic) {
                                TextButton(enabled = !busy && !reading && !uncertain && open.writable,
                                    onClick = { dialog = Dialog.Move(e, path.toList()) }) { Text(tr("Перенести…", "Move…")) }
                                if (!e.folder) TextButton(enabled = !busy && !reading && !uncertain && open.writable && selectedFiles.size == 1,
                                    onClick = { selectedFiles.singleOrNull()?.let { source -> dialog = Dialog.Replace(e, path.toList(), source) } }) {
                                    Text(tr("Заменить содержимое…", "Replace content…"))
                                }
                            }
                            TextButton(enabled = !busy && !reading && !uncertain && open.writable, onClick = { dialog = Dialog.Rename(e) }) { Text(tr("Имя", "Rename")) }
                            TextButton(enabled = !busy && !reading && !uncertain && open.writable, onClick = { dialog = Dialog.Delete(e) }) { Text(tr("Удалить", "Delete")) }
                        }
                        }
                    }
                item { Text(tr("«Расшифровать» создаёт новый файл в текущей папке файлового менеджера${here?.let { " (" + displayName(it.name) + ")" } ?: ""}; существующие файлы не перезаписываются.",
                    "\"Decrypt\" creates a new file in the file manager's current folder${here?.let { " (" + displayName(it.name) + ")" } ?: ""}; existing files are not overwritten."),
                    style = MaterialTheme.typography.bodySmall) }
            }
            }
        }
    }

    when (val d = dialog) {
        null -> Unit
        is Dialog.ImportMove -> vault?.let { opened ->
            VaultImportMoveDialog(d.sources, !busy && !reading && !uncertain && opened.writable, onClose = { dialog = null }) {
                val requests = d.sources.map { it.location to randomRequestId() }
                dialog = null
                act {
                    protectedWrite(session) {
                        var result: CoreResult<Unit> = CoreResult.Value(Unit)
                        for ((source, requestId) in requests) {
                            when (val moved = gateway.moveIn(opened.handle, d.path, source, requestId)) {
                                is CoreResult.Value -> { importReports = importReports + moved.value; session.refreshFileTabs() }
                                is CoreResult.Failure -> { result = moved; break }
                            }
                        }
                        result
                    }
                }
            }
        }
        Dialog.CreateText -> vault?.let { opened -> CreateTextDialog(listing?.entries.orEmpty().map { it.name }.toSet(),
            onDismiss = { dialog = null }) { name ->
            dialog = null
            act { protectedWrite(session) { gateway.textCreate(opened.handle, path.toList(), name, "") } }
        } }
        is Dialog.Access -> PasswordDialog(core = session.core, create = d.create,
            context = (if (format == FolderFormat.Cryptomator) "Cryptomator" else "Paranoic 1.8") + " · " +
                displayName(files.spaces.spaceOf(d.target.location)?.name.orEmpty()) + " / " + displayName(d.target.location.path),
            onDismiss = { dialog = null }) { name, password ->
            dialog = null
            val node = d.target
            if (busy || uncertain) return@PasswordDialog
            if (d.create && (!session.vaultCatalog.loaded || session.vaultCatalog.failed)) {
                fault = CoreFault.Unavailable; return@PasswordDialog
            }
            setBusy(true); fault = null; recoveryAccess = false; completed = null
            scope.launch {
                try {
                    val r = safe { if (d.create) gateway.create(node.location, name, password) else gateway.open(node.location, password) }
                    when (r) {
                        is CoreResult.Value -> {
                            val location = if (d.create) Location(node.location.storageId,
                                listOf(node.location.path, name).filter { it.isNotEmpty() }.joinToString("/")) else node.location
                            if (d.original != null) session.vaultCatalog.relocate(d.original, location,
                                if (format == FolderFormat.Cryptomator) VaultKind.Cryptomator else VaultKind.Paranoic)
                            else session.vaultCatalog.remember(SavedVault(location,
                                if (format == FolderFormat.Cryptomator) VaultKind.Cryptomator else VaultKind.Paranoic,
                                (if (d.create) name else node.name).filterNot { it.isISOControl() }.take(120).ifBlank { "Paranoic" }))
                            val preferences = if (d.create) null else d.original ?: session.vaultCatalog.snapshot.entries.firstOrNull { it.location == node.location }
                            val writable = canWriteOpenedVault(files.spaces.spaceOf(node.location)?.writable == true, preferences,
                                session.vaultCatalog.loaded, session.vaultCatalog.failed)
                            if (!writable) gateway.restrictWrites(r.value)
                            val v = OpenVault(r.value, if (d.create) name else node.name, writable, location)
                            vault = v; reload(v, emptyList())
                        }
                        is CoreResult.Failure -> { fault = r.fault; uncertain = unknownOutcome(r.fault) }
                    }
                    if (d.create) session.refresh()
                } finally { setBusy(false) }
            }
        }
        is Dialog.RecoveryOpen -> VaultRecoveryOpenDialog(core, d.target.name,
            canReset = canWriteOpenedVault(files.spaces.spaceOf(d.target.location)?.writable == true,
                session.vaultCatalog.snapshot.entries.firstOrNull { it.location == d.target.location },
                session.vaultCatalog.loaded, session.vaultCatalog.failed),
            onClose = { dialog = null }) { recovery, newPassword ->
            dialog = null
            if (busy || uncertain) return@VaultRecoveryOpenDialog
            setBusy(true); fault = null; recoveryAccess = true; completed = null
            scope.launch {
                try {
                    when (val r = safe { gateway.openRecovery(d.target.location, recovery) }) {
                        is CoreResult.Value -> {
                            val preferences = session.vaultCatalog.snapshot.entries.firstOrNull { it.location == d.target.location }
                            val writable = canWriteOpenedVault(files.spaces.spaceOf(d.target.location)?.writable == true, preferences,
                                session.vaultCatalog.loaded, session.vaultCatalog.failed)
                            if (!writable) gateway.restrictWrites(r.value)
                            session.vaultCatalog.remember(SavedVault(d.target.location, VaultKind.Paranoic,
                                d.target.name.filterNot { it.isISOControl() }.take(120).ifBlank { "Paranoic" }))
                            val v = OpenVault(r.value, d.target.name, writable, d.target.location)
                            vault = v
                            if (newPassword != null) {
                                when (val reset = safe { gateway.resetPassword(v.handle, recovery, newPassword) }) {
                                    is CoreResult.Value -> { completed = FolderWriteNotice.Password; reload(v, emptyList()); session.refresh() }
                                    is CoreResult.Failure -> { fault = reset.fault; uncertain = unknownOutcome(reset.fault) }
                                }
                            } else reload(v, emptyList())
                        }
                        is CoreResult.Failure -> { fault = r.fault; uncertain = unknownOutcome(r.fault) }
                    }
                } finally { setBusy(false) }
            }
        }
        Dialog.ChangePassword -> vault?.let { opened ->
            VaultChangePasswordDialog(session.core, opened.name, format == FolderFormat.Cryptomator, onClose = { dialog = null }) { old, new ->
                dialog = null
                act(FolderWriteNotice.Password) { gateway.changePassword(opened.handle, old, new) }
            }
        }
        Dialog.RecoverySetup -> vault?.let { opened ->
            VaultRecoverySetupDialog(opened.name, onClose = { dialog = null }) { password ->
                dialog = null
                act {
                    gateway.setRecovery(opened.handle, password).also { result ->
                        if (result is CoreResult.Value) recoveryText = result.value
                    }
                }
            }
        }
        is Dialog.Move -> vault?.let { opened ->
            VaultMoveDialog(gateway, opened.handle, d.path, d.entry, onClose = { dialog = null }) { targetPath ->
                dialog = null
                act(FolderWriteNotice.Move) { gateway.move(opened.handle, d.path, d.entry, targetPath) }
            }
        }
        is Dialog.Replace -> AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { dialog = null },
            title = { Text(tr("Заменить содержимое файла?", "Replace file contents?")) },
            text = { Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("В хранилище: ", "In vault: ") + vaultPath(d.path + d.entry.name))
                Text(tr("Новый источник: ", "New source: ") + displayName(files.spaces.spaceOf(d.source.location)?.name.orEmpty()) + " /" + displayName(d.source.location.path))
                Text(tr("Размер выбранного источника: ", "Selected source size: ") + formatSize(d.source.bytes))
                Text(tr("Текущее содержимое файла в папке 1.8 будет заменено данными источника, прочитанными при выполнении. Имя сохранится; исходный файл не удаляется. Изменение цели или источника после начала операции приводит к отказу (Changed). Старое содержимое может быть недоступно после успешной замены.",
                    "The current contents in the 1.8 vault will be replaced with source bytes read during execution. The name stays; the source is kept. A target or source change after the operation starts causes Changed. Old contents may be unavailable after a successful replacement."))
            } },
            confirmButton = { TextButton(enabled = !busy && !uncertain, onClick = {
                dialog = null
                vault?.let { opened -> act(FolderWriteNotice.Replace) { gateway.replace(opened.handle, d.path, d.entry.name, d.source.location) } }
            }) { Text(tr("Заменить", "Replace")) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text(tr("Отмена", "Cancel")) } })
        Dialog.Mkdir -> NameOnlyDialog(tr("Новая папка", "New folder"), "", { dialog = null }) { name ->
            dialog = null; vault?.let { v -> act { gateway.mkdir(v.handle, path, name) } }
        }
        is Dialog.Rename -> NameOnlyDialog(tr("Новое имя", "New name"), d.entry.name, { dialog = null }) { name ->
            dialog = null; vault?.let { v -> act { gateway.rename(v.handle, path, d.entry.name, name, d.entry.folder) } }
        }
        is Dialog.Export -> AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { dialog = null },
            title = { Text(tr("Создать незашифрованную копию?", "Create an unencrypted copy?")) },
            text = { Text(displayName(d.entry.name) + " → " + displayName(d.parent.name) + "\n" + tr("Копия доступна вне хранилища. Существующий файл не заменяется.", "The copy is accessible outside the vault. Existing files are not replaced.")) },
            confirmButton = { TextButton(enabled = !busy, onClick = { dialog = null; vault?.let { v -> act { gateway.export(v.handle, path, d.entry.name, d.parent.location).also { session.refresh() } } } }) { Text(tr("Экспортировать", "Export")) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text(tr("Отмена", "Cancel")) } })
        is Dialog.Delete -> AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { dialog = null },
            title = { Text(tr("Удалить «${displayName(d.entry.name)}»?", "Delete \"${displayName(d.entry.name)}\"?")) },
            text = { Text(if (d.entry.folder && format == FolderFormat.Cryptomator) tr("Папка будет удалена со всем содержимым. Это не отметка пустой папки. Восстановление не гарантируется.", "The folder and all its contents will be deleted. This is not an empty-folder-only operation. Recovery is not guaranteed.")
                else if (d.entry.folder) tr("Папка удаляется только пустой.", "Only an empty folder is deleted.")
                else tr("Зашифрованный файл будет удалён. Расшифрованные копии вне папки не затрагиваются.",
                    "The encrypted file is deleted. Decrypted copies outside the folder are not affected.")) },
            confirmButton = { TextButton(onClick = {
                dialog = null; vault?.let { v -> act { gateway.delete(v.handle, path, d.entry.name, d.entry.folder) } }
            }) { Text(tr("Удалить", "Delete")) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text(tr("Отмена", "Cancel")) } })
    }
    recoveryText?.let { text ->
        if (vault != null) VaultRecoveryKeyDialog(core, text) { recoveryText = null }
    }
    viewing?.let { selected -> vault?.let { opened ->
        VaultFileWindow(session, gateway, opened.handle, selected.second, selected.first, opened.writable,
            secureImage, mediaPlayer, onBusy = ::setBusy, onClose = { viewing = null; reload(opened, path) })
    } }
    }
}

private sealed interface Dialog {
    data class ImportMove(val sources: List<FileItem>, val path: List<String>) : Dialog
    data object CreateText : Dialog
    data class Access(val create: Boolean, val target: FolderNode, val original: SavedVault? = null) : Dialog
    data class RecoveryOpen(val target: FolderNode) : Dialog
    data object ChangePassword : Dialog
    data object RecoverySetup : Dialog
    data class Move(val entry: VaultEntry, val path: List<String>) : Dialog
    data class Replace(val entry: VaultEntry, val path: List<String>, val source: FileItem) : Dialog
    data object Mkdir : Dialog
    data class Rename(val entry: VaultEntry) : Dialog
    data class Delete(val entry: VaultEntry) : Dialog
    data class Export(val entry: VaultEntry, val parent: FolderNode) : Dialog
}

private fun unknownOutcome(fault: CoreFault) = fault in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
    CoreFault.Cancelled, CoreFault.Disconnected)

private enum class FolderWriteNotice { Password, Move, Replace }

private suspend fun <T> safe(block: suspend () -> CoreResult<T>): CoreResult<T> = try { block() }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }

@Composable
private fun formatSize(bytes: ULong): String = when {
    bytes < 1024uL -> tr("$bytes байт", "$bytes B")
    bytes < 1024uL * 1024uL -> tr("${bytes / 1024uL} КиБ", "${bytes / 1024uL} KiB")
    bytes < 1024uL * 1024uL * 1024uL -> tr("${bytes / (1024uL * 1024uL)} МиБ", "${bytes / (1024uL * 1024uL)} MiB")
    else -> tr("${bytes / (1024uL * 1024uL * 1024uL)} ГиБ", "${bytes / (1024uL * 1024uL * 1024uL)} GiB")
}

@Composable
private fun PasswordDialog(core: CoreGateway, create: Boolean, context: String, onDismiss: () -> Unit, onConfirm: (name: String, password: String) -> Unit) {
    var name by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    var stage by remember { mutableIntStateOf(0) }
    val reviewing = stage == 2
    DisposableEffect(Unit) { onDispose { password = ""; repeat = "" } }
    val valid = password.isNotEmpty() && (!create || plausibleName(name) && password == repeat && password.length >= 12)
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { password = ""; repeat = ""; onDismiss() },
        title = { Text(if (create) tr("Новая зашифрованная папка", "New encrypted folder") else tr("Открыть зашифрованную папку", "Open encrypted folder")) },
        text = {
            Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(context, style = MaterialTheme.typography.bodySmall)
                if (create) Text(when (stage) {
                    0 -> tr("Шаг 4 из 6 · Пароль", "Step 4 of 6 · Password")
                    1 -> tr("Шаг 5 из 6 · Параметры формата", "Step 5 of 6 · Format settings")
                    else -> tr("Шаг 6 из 6 · Проверка", "Step 6 of 6 · Review")
                }, style = MaterialTheme.typography.labelLarge)
                if (create && stage == 1) {
                    Text(tr("Используются защитные параметры, заданные движком для выбранного формата. Размер хранилища растёт с добавлением файлов; фиксированная ёмкость не выделяется.",
                        "The engine uses the protective defaults for the selected format. Storage grows as files are added; no fixed capacity is preallocated."))
                    Text(tr("Пароль не сохраняется в карточке. Место и имя можно проверить перед созданием. Изменение технических параметров этим мастером не поддерживается.",
                        "The card does not store your password. Review the location and name before creation. This wizard does not expose changes to technical parameters."))
                    TextButton(onClick = { stage = 0 }) { Text(tr("Назад к паролю", "Back to password")) }
                } else if (reviewing) {
                    Text(tr("Проверьте перед созданием", "Review before creating"), style = MaterialTheme.typography.titleMedium)
                    Text(displayName(name))
                    Text(tr("В выбранном месте появится новая зашифрованная подпапка. Существующие файлы не заменяются. Пароль в сводке не показывается.",
                        "A new encrypted subfolder will be created at this location. Existing files are not replaced. The password is not shown in this review."))
                    TextButton(onClick = { stage = 1 }) { Text(tr("Назад к параметрам", "Back to settings")) }
                } else {
                if (create) OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
                    modifier = Modifier.fillMaxWidth().fieldError(nameErrorText(), name.isNotEmpty() && !plausibleName(name)),
                    label = { Text(tr("Имя папки", "Folder name")) }, isError = name.isNotEmpty() && !plausibleName(name),
                    supportingText = { if (name.isNotEmpty() && !plausibleName(name)) Text(nameErrorText()) })
                OutlinedTextField(password, { if (it.length <= 4096) password = it }, singleLine = true, label = { Text(tr("Пароль", "Password")) },
                    visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                if (create) {
                    PasswordTools(core, password) { password = it; repeat = "" }
                    OutlinedTextField(repeat, { if (it.length <= 4096) repeat = it }, singleLine = true, label = { Text(tr("Повторите пароль", "Repeat password")) },
                        modifier = Modifier.fieldError(passwordMismatchText(), repeat.isNotEmpty() && repeat != password),
                        visualTransformation = PasswordVisualTransformation(), isError = repeat.isNotEmpty() && repeat != password,
                        supportingText = { if (repeat.isNotEmpty() && repeat != password) Text(passwordMismatchText()) },
                        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                    Text(tr("Не меньше 12 символов; лучше фраза из нескольких слов. Забытый пароль восстановить нельзя.",
                        "At least 12 characters; a phrase of several words is better. A forgotten password cannot be recovered."),
                        style = MaterialTheme.typography.bodySmall)
                }
                }
            }
        },
        confirmButton = { TextButton(enabled = valid, onClick = {
            if (create && !reviewing) stage++
            else { val p = password; password = ""; repeat = ""; onConfirm(name, p) }
        }) { Text(if (create && stage == 0) tr("Далее", "Next") else if (create && stage == 1) tr("Проверить", "Review") else if (create) tr("Создать", "Create") else tr("Открыть", "Open")) } },
        dismissButton = { TextButton(onClick = { password = ""; repeat = ""; onDismiss() }) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
private fun NameOnlyDialog(title: String, initial: String, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by remember { mutableStateOf(initial) }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = onDismiss, title = { Text(title) },
        text = { OutlinedTextField(name, { if (it.length <= 255) name = it }, singleLine = true,
            modifier = Modifier.fieldError(nameErrorText(), name.isNotEmpty() && !plausibleName(name)),
            label = { Text(tr("Имя", "Name")) }, isError = name.isNotEmpty() && !plausibleName(name),
            supportingText = { if (name.isNotEmpty() && !plausibleName(name)) Text(nameErrorText()) }) },
        confirmButton = { TextButton(enabled = plausibleName(name) && name != initial, onClick = { onConfirm(name) }) { Text(tr("Сохранить", "Save")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
