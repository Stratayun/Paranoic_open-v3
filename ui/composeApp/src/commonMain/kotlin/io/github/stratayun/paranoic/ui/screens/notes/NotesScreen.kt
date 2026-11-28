package io.github.stratayun.paranoic.ui.screens.notes

import io.github.stratayun.paranoic.resources.*
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import io.github.stratayun.paranoic.ui.components.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.PasswordTools
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.components.LabeledCheckbox
import io.github.stratayun.paranoic.ui.components.LabeledSwitch
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Protected notes (1.9). Isolated from the file manager and the cloud, never
 * synchronized. Decrypted text lives only in this screen's memory and is dropped
 * on lock (Quick Lock bumps `session.secretsEpoch`). No auto-save, no auto-retry. */
@Composable
@OptIn(ExperimentalLayoutApi::class)
fun NotesScreen(session: AppSession, biometric: NotesBiometric = NoNotesBiometric,
    attachmentPicker: NotesAttachmentPicker = NoNotesAttachmentPicker,
    attachmentImage: @Composable (ByteArray, Modifier) -> Unit = { _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) },
    onDiagnosticsCleared: suspend () -> Boolean = { true }, exitGuard: DocumentExitGuard? = null,
    incoming: IncomingDocuments = NoIncomingDocuments,
    vaultMediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit = { _, _, _, modifier -> FaultNotice(CoreFault.Unsupported, modifier) }) {
    val core = session.core
    val picker = if (attachmentPicker === NoNotesAttachmentPicker)
        remember(core, session.secretsEpoch) { CoreNotesAttachmentPicker(core) }
    else attachmentPicker
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<NotesStatus?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var notesBusy by remember { mutableStateOf(false) }
    var personalBusy by remember { mutableStateOf(false) }
    val busy = notesBusy || personalBusy
    var personalFiles by remember { mutableStateOf(false) }
    var weakSafePassword by remember(session.secretsEpoch) { mutableStateOf(false) }
    var tabs by remember { mutableStateOf<List<NoteTab>>(emptyList()) }
    var dirty by remember { mutableStateOf(false) }
    var notesLoaded by remember { mutableStateOf(false) }
    var preview by remember { mutableStateOf<NoteAttachment?>(null) }
    var biometricFailed by remember { mutableStateOf(false) }
    DisposableEffect(biometric) { onDispose { biometric.cancel() } }
    var saveUncertain by remember { mutableStateOf(false) }
    var diagnosticCleanupFailed by remember { mutableStateOf(false) }
    var importing by remember { mutableStateOf(false) }
    var exporting by remember { mutableStateOf(false) }
    var importReport by remember { mutableStateOf<NotesImportReport?>(null) }
    var exportReport by remember { mutableStateOf<NotesExportReport?>(null) }
    var checkingWrite by remember { mutableStateOf(false) }
    var duressSettingFault by remember { mutableStateOf(false) }
    SideEffect {
        exitGuard?.busy = busy
        exitGuard?.dirty = dirty
        exitGuard?.uncertain = saveUncertain || session.protectedWrite.phase != ProtectedWritePhase.Ready
    }
    DisposableEffect(exitGuard) { onDispose {
        exitGuard?.busy = false; exitGuard?.dirty = false; exitGuard?.uncertain = false; exitGuard?.cancel()
    } }
    LaunchedEffect(fault) {
        if (fault == CoreFault.Wiped) {
            tabs = emptyList(); dirty = false; notesLoaded = false; preview = null; picker.clear()
            diagnosticCleanupFailed = !onDiagnosticsCleared()
        }
    }

    suspend fun reloadStatus(): Boolean {
        return when (val r = safe { core.notesStatus() }) {
            is CoreResult.Value -> { status = r.value; true }
            is CoreResult.Failure -> { fault = r.fault; false }
        }
    }
    suspend fun loadNotes(): Boolean {
        return when (val r = safe { core.notesRead() }) {
            is CoreResult.Value -> { tabs = r.value; dirty = false; notesLoaded = true; saveUncertain = false; true }
            is CoreResult.Failure -> { fault = r.fault; if (r.fault == CoreFault.Locked) reloadStatus(); false }
        }
    }
    suspend fun persistNotes(): Boolean {
        if (!notesLoaded || saveUncertain) return false
        return when (val result = safe { core.notesSave(tabs) }) {
            is CoreResult.Value -> { dirty = false; saveUncertain = false; true }
            is CoreResult.Failure -> {
                fault = result.fault
                saveUncertain = result.fault in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
                    CoreFault.Cancelled, CoreFault.Disconnected)
                false
            }
        }
    }
    suspend fun authenticated() {
        // Apply the same reset after every successful unlock; no stale preview,
        // operation report or draft may disclose a previous protected session.
        tabs = emptyList(); dirty = false; notesLoaded = false; preview = null; personalFiles = false
        importing = false; exporting = false; checkingWrite = false
        importReport = null; exportReport = null; diagnosticCleanupFailed = false; biometricFailed = false
        onDiagnosticsCleared()
        reloadStatus(); loadNotes()
    }
    /** One action at a time; every result is shown, nothing is repeated by itself. */
    fun act(block: suspend () -> Unit) {
        if (busy) return
        notesBusy = true
        exitGuard?.busy = true
        fault = null
        duressSettingFault = false
        scope.launch { try { block() } finally { notesBusy = false; exitGuard?.busy = personalBusy } }
    }
    fun quick(enabling: Boolean) {
        val requestEpoch = session.secretsEpoch
        act {
            biometricFailed = false
            if (!biometric.authenticate(enabling) || requestEpoch != session.secretsEpoch) {
                biometric.cancel()
                biometricFailed = true
                return@act
            }
            try {
                when (val result = safe { if (enabling) core.notesQuickEnable() else core.notesQuickUnlock() }) {
                    is CoreResult.Value -> {
                        if (!enabling) authenticated() else reloadStatus()
                    }
                    is CoreResult.Failure -> { fault = result.fault; reloadStatus() }
                }
            } finally { biometric.cancel() }
        }
    }
    LaunchedEffect(session.secretsEpoch) {
        tabs = emptyList(); dirty = false; notesLoaded = false; preview = null
        reloadStatus()
    }

    val current = status
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.heightIn(max = 160.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (biometricFailed) Text(tr("Биометрия отменена или ключ недоступен. Используйте пароль; эта попытка не увеличивает счётчик автостирания.",
            "Biometrics cancelled or the key is unavailable. Use your password; this attempt does not count toward auto-wipe."))
        fault?.let { FaultNotice(it, message = if (duressSettingFault && it == CoreFault.InvalidName)
            tr("Пароль под принуждением должен отличаться от основного.", "The duress password must differ from the main password.") else it.label()) }
        if (session.protectedWrite.phase == ProtectedWritePhase.Unknown) {
            Text(tr("Результат записи неизвестен. Проверьте тетрадь и папку; повтор заблокирован.",
                "Write outcome unknown. Check the notebook and folder; retry is blocked."),
                color = MaterialTheme.colorScheme.error)
            TextButton(enabled = !busy, onClick = { checkingWrite = true }) { Text(tr("Проверить состояние…", "Reconcile state…")) }
        }
        importReport?.let { Text(tr("Импортировано: вкладок ${it.tabs}, заметок ${it.notes}; пропущено ${it.skipped}.",
            "Imported: ${it.tabs} tabs, ${it.notes} notes; ${it.skipped} skipped.")) }
        exportReport?.let { Text(tr("Экспортировано открытых файлов: ${it.files}. Вложений осталось внутри: ${it.skippedAttachments}.",
            "Plaintext files exported: ${it.files}. Attachments kept inside: ${it.skippedAttachments}.")) }
        if (diagnosticCleanupFailed) Text(tr("Часть журнала не очистилась — повторите очистку в Диагностике.",
            "Part of the journal was not cleared — clear it again in Diagnostics."), color = MaterialTheme.colorScheme.error)
        if (saveUncertain) Text(tr("Результат сохранения неизвестен. Откройте заметки заново и проверьте текст.",
            "Save outcome unknown. Reopen the notes and check the text."),
            color = MaterialTheme.colorScheme.error)
        }
        when {
            !core.connected -> FaultNotice(CoreFault.Disconnected)
            current == null -> CircularProgressIndicator()
            !current.setUp -> CreateNotes(session, current, busy || session.protectedWrite.phase != ProtectedWritePhase.Ready) { password ->
                act {
                    when (val r = safe { core.notesCreate(password) }) {
                        is CoreResult.Value -> { reloadStatus(); loadNotes() }
                        is CoreResult.Failure -> { fault = r.fault; reloadStatus() }
                    }
                }
            }
            !current.unlocked -> {
                if (current.quickEnabled && current.quickAvailable && biometric.available) {
                    FilledTonalButton(enabled = !busy, onClick = { quick(false) }, modifier = Modifier.fillMaxWidth().height(56.dp)) {
                        AppIcon(AppSymbol.Fingerprint); Spacer(Modifier.width(8.dp)); Text(tr("Открыть по биометрии", "Unlock with biometrics"))
                    }
                }
                UnlockNotes(current, busy, onRefresh = { act { reloadStatus() } }) { password ->
                    act {
                        when (val r = safe { core.notesUnlock(password) }) {
                            is CoreResult.Value -> {
                                // Audit 26.11: depends only on the typed password, the same for every way in.
                                weakSafePassword = (core.passwordFeedback(password) as? CoreResult.Value)?.value?.strength == PasswordStrength.Weak
                                // The same policy follows EVERY successful authentication.
                                // Never ask Core whether a duress password was used.
                                authenticated()
                            }
                            is CoreResult.Failure -> { fault = r.fault; reloadStatus() }
                        }
                    }
                }
            }
            else -> { Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                LaunchedEffect(current.unlocked) { if (!notesLoaded) act { loadNotes() } }
                if (weakSafePassword) Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Column(Modifier.fillMaxWidth().padding(12.dp)) {
                        Text(tr("Пароль Сейфа слабее нынешнего правила (от 12 символов). Смените его в настройках Сейфа ниже.",
                            "The Safe password is weaker than the current rule (12+ characters). Change it in the Safe settings below."))
                        TextButton(onClick = { weakSafePassword = false }) { Text(tr("Понятно", "Got it")) }
                    }
                }
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(top = 8.dp)) {
                    SegmentedButton(selected = !personalFiles, enabled = !busy, onClick = { personalFiles = false },
                        shape = SegmentedButtonDefaults.itemShape(0, 2), icon = { AppIcon(AppSymbol.Notes) }) { Text(tr("Заметки", "Notes")) }
                    SegmentedButton(selected = personalFiles, enabled = !busy && notesLoaded && !saveUncertain && session.protectedWrite.phase == ProtectedWritePhase.Ready,
                        onClick = { act { if (persistNotes()) { preview = null; personalFiles = true } } },
                        shape = SegmentedButtonDefaults.itemShape(1, 2), icon = { AppIcon(AppSymbol.Folder) }) { Text(tr("Личная папка", "Personal folder")) }
                }
                if (personalFiles) PersonalFilesPane(session, enabled = notesLoaded && !saveUncertain, incoming = incoming,
                    secureImage = attachmentImage, mediaPlayer = vaultMediaPlayer, onBusy = { value ->
                    personalBusy = value; exitGuard?.busy = notesBusy || value
                }, onLock = {
                    preview = null; picker.clear(); biometric.cancel()
                    tabs = emptyList(); dirty = false; notesLoaded = false
                    session.pause(); session.resume()
                })
                else {
                if (!notesLoaded) {
                    CircularProgressIndicator()
                    TextButton(enabled = !busy, onClick = { act { loadNotes() } }) { Text(tr("Загрузить заметки", "Load notes")) }
                } else Notebook(tabs, dirty, busy, writeBlocked = saveUncertain || session.protectedWrite.phase != ProtectedWritePhase.Ready,
                    pickerAvailable = picker.available && !picker.picking,
                    onChange = { if (!busy && !saveUncertain && session.protectedWrite.phase == ProtectedWritePhase.Ready) {
                        tabs = it; dirty = true; exitGuard?.dirty = true
                    } },
                    onSave = {
                        act { persistNotes() }
                    },
                    onLock = {
                        preview = null; picker.clear(); biometric.cancel()
                        tabs = emptyList(); dirty = false; notesLoaded = false
                        session.pause(); session.resume()
                    },
                    onPick = { noteId -> act { if (persistNotes()) picker.pick(noteId) } },
                    onPreview = { preview = it },
                    onRemoveAttachment = { noteId, attachmentId ->
                        act {
                            tabs = tabs.map { tab -> tab.copy(notes = tab.notes.map { note ->
                                if (note.id == noteId) note.copy(attachments = note.attachments.filterNot { it.id == attachmentId }, modifiedMs = nowMs()) else note
                            }) }
                            dirty = true
                            persistNotes()
                        }
                    })
                if (notesLoaded) (picker as? CoreNotesAttachmentPicker)?.Dialog(session.files.spaces)
                picker.fault?.let { pickerFault ->
                    FaultNotice(pickerFault, message = if (pickerFault == CoreFault.Limit) attachmentLimitText() else pickerFault.label())
                    TextButton(onClick = picker::clear) { Text(tr("Закрыть сообщение", "Dismiss")) }
                }
                val selected = picker.pending
                if (selected != null && notesLoaded) {
                    val target = tabs.flatMap { it.notes }.firstOrNull { it.id == selected.noteId }
                    if (target == null) {
                        // Applies equally to deleted notes and a new empty notebook.
                        // Never display a capability from a previous secret session.
                        LaunchedEffect(selected) { picker.clear() }
                    } else {
                    Text(tr("Выбран файл: ", "Selected file: ") + displayName(selected.name))
                    Text(tr("Для заметки: ", "For note: ") + displayName(target.title.ifBlank { tr("Без заголовка", "Untitled") }))
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        TextButton(enabled = !busy && !saveUncertain && session.protectedWrite.phase == ProtectedWritePhase.Ready,
                            onClick = { act {
                                if (target.attachments.size >= 32) { fault = CoreFault.Limit; return@act }
                                when (val read = safe { selected.source.read() }) {
                                    is CoreResult.Failure -> fault = read.fault
                                    is CoreResult.Value -> try {
                                        if (read.value.size > NOTE_ATTACHMENT_MAX_BYTES) { fault = CoreFault.Limit; return@act }
                                        when (val added = safe { core.notesAttachmentAdd(selected.name, read.value) }) {
                                            is CoreResult.Failure -> {
                                                fault = added.fault
                                                saveUncertain = added.fault in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
                                                    CoreFault.Cancelled, CoreFault.Disconnected)
                                            }
                                            is CoreResult.Value -> {
                                                tabs = tabs.map { tab -> tab.copy(notes = tab.notes.map { note ->
                                                    if (note.id == selected.noteId) note.copy(attachments = note.attachments + added.value, modifiedMs = nowMs()) else note
                                                }) }
                                                dirty = true
                                                picker.clear()
                                                persistNotes()
                                            }
                                        }
                                    } finally { read.value.fill(0) }
                                }
                            } }) { Text(tr("Добавить в заметку", "Add to note")) }
                        TextButton(enabled = !busy, onClick = picker::clear) { Text(tr("Отмена", "Cancel")) }
                    }
                }
                }
                }
                run {
                    val ready = !busy && !saveUncertain && notesLoaded && session.protectedWrite.phase == ProtectedWritePhase.Ready
                    val scheme = MaterialTheme.colorScheme
                    SectionLabel(tr("Обмен", "Exchange"))
                    GroupedRow(tr("Импорт .txt / .md", "Import .txt / .md"), GroupPosition.First, enabled = ready,
                        subtitle = tr("Добавить текстовые файлы как заметки", "Add text files as notes"),
                        leading = { BrandIcon(Res.drawable.ic_downloads, size = 52.dp) },
                        onClick = { importing = true })
                    Spacer(Modifier.height(2.dp))
                    GroupedRow(tr("Экспорт открытых копий", "Export plaintext copies"), GroupPosition.Last, enabled = ready,
                        subtitle = tr("Копии будут НЕ зашифрованы", "Copies are NOT encrypted"),
                        leading = { BrandIcon(Res.drawable.ic_export, size = 52.dp) },
                        onClick = { exporting = true })
                }
                // The unlocked pane scrolls as one unit; nested lists have finite height.
                Column(Modifier.fillMaxWidth(),
                    verticalArrangement = Arrangement.spacedBy(12.dp)) {
                NotesSettings(session, current, busy || saveUncertain || !notesLoaded || session.protectedWrite.phase != ProtectedWritePhase.Ready, biometric.available,
                    onQuick = { enable ->
                        if (enable) quick(true)
                        else act { (safe { core.notesQuickDisable() } as? CoreResult.Failure)?.let { fault = it.fault }; reloadStatus() }
                    },
                    onAutoWipe = { enabled -> act { safe { core.notesSetAutoWipe(enabled) }.also { if (it is CoreResult.Failure) fault = it.fault }; reloadStatus() } },
                    onDuress = { secret -> act {
                        if (!persistNotes()) return@act
                        duressSettingFault = true
                        when (val result = protectedWrite(session) { core.notesSetDuress(secret) }) {
                            is CoreResult.Value -> Unit
                            is CoreResult.Failure -> fault = result.fault
                        }
                    } },
                    onChangePassword = { old, new ->
                        // A password change also replaces the notes key and turns quick unlock off.
                        act {
                            if (!persistNotes()) return@act
                            preview = null
                            val result = protectedWrite(session) { core.notesChangePassword(old, new) }
                            if (result is CoreResult.Failure) fault = result.fault
                            // Rotation rewrites attachment IDs. Never save the old model again,
                            // even if reading the new notebook fails or the write reply was lost.
                            if (result is CoreResult.Value || result is CoreResult.Failure && result.fault in setOf(
                                    CoreFault.OutcomeUnknown, CoreFault.Unavailable, CoreFault.Cancelled, CoreFault.Disconnected)) {
                                tabs = emptyList(); dirty = false; notesLoaded = false; saveUncertain = false
                            }
                            reloadStatus()
                            if (result is CoreResult.Value) loadNotes()
                        }
                    },
                    onClear = {
                        act {
                            val r = protectedWrite(session) { core.safeClear() }
                            // A removal failure can follow successful key destruction.
                            // Drop the old model on every outcome; never repeat Clear.
                            tabs = emptyList(); dirty = false; notesLoaded = false; preview = null
                            personalFiles = false; picker.clear()
                            when (r) {
                                is CoreResult.Value -> Unit
                                is CoreResult.Failure -> fault = r.fault
                            }
                            diagnosticCleanupFailed = !onDiagnosticsCleared()
                            reloadStatus()
                        }
                    })
                }
                }
            }
        }
    }
    preview?.let { attachment ->
        if (current?.unlocked == true) NoteAttachmentPreview(core, attachment, attachmentImage) { preview = null }
    }
    if (importing && current?.unlocked == true) NotesImportDialog(core, session.files.spaces, tabs,
        onDismiss = { importing = false }, onImport = { sources, title, into ->
            importing = false
            act {
                if (!persistNotes()) return@act
                importReport = null; exportReport = null; preview = null
                val result = protectedWrite(session) { core.notesImport(sources, title, into) }
                // Core may already have added IDs. The old model must never be saved over it.
                tabs = emptyList(); dirty = false; notesLoaded = false
                when (result) {
                    is CoreResult.Value -> { importReport = result.value; loadNotes() }
                    is CoreResult.Failure -> { fault = result.fault; saveUncertain = result.fault.hasUnknownWriteOutcome() }
                }
                reloadStatus()
            }
        })
    if (exporting && current?.unlocked == true) NotesExportDialog(core, session.files.spaces,
        onDismiss = { exporting = false }, onExport = { parent, name ->
            exporting = false
            act {
                if (!persistNotes()) return@act
                importReport = null; exportReport = null
                when (val result = protectedWrite(session) { core.notesExport(parent, name) }) {
                    is CoreResult.Value -> { exportReport = result.value; session.refreshFileTabs() }
                    is CoreResult.Failure -> fault = result.fault
                }
            }
        })
    if (checkingWrite) {
        var checked by remember { mutableStateOf(false) }
        AlertDialog(onDismissRequest = { checkingWrite = false }, title = { Text(tr("Проверка после потерянного ответа", "Reconcile a lost reply")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Проверьте папку назначения: могла остаться открытая копия. Кнопка ничего не повторяет и не удаляет.",
                    "Check the destination: an unencrypted copy may remain. This button neither retries nor deletes."))
                LabeledCheckbox(tr("Проверил(а) состояние файлов; разрешаю перечитать заметки", "I inspected the files; reload the notebook"), checked, onChange = { checked = it })
            } },
            confirmButton = { TextButton(enabled = checked && !busy, onClick = {
                checkingWrite = false
                act {
                    if (reloadStatus() && (status?.unlocked != true || loadNotes())) {
                        session.acknowledgeProtectedWrite(); saveUncertain = false
                    }
                }
            }) { Text(tr("Перечитать состояние", "Reload state")) } },
            dismissButton = { TextButton(onClick = { checkingWrite = false }) { Text(tr("Отмена", "Cancel")) } })
    }
}

@Composable
private fun attachmentLimitText() = tr("Лимиты вложений: 15 МиБ на файл, 256 МиБ всего, 32 вложения на заметку.",
    "Attachment limits: 15 MiB per file, 256 MiB total, 32 attachments per note.")

private suspend fun <T> safe(block: suspend () -> CoreResult<T>): CoreResult<T> = try { block() }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { CoreResult.Failure(CoreFault.Unavailable) }

@Composable
private fun GuaranteeText(guarantee: KeyGuarantee) {
    Text(when (guarantee) {
        KeyGuarantee.HardwareKey -> tr("Заметки защищены паролем и ключом устройства с подтверждённой защитой от восстановления удалённого ключа.",
            "Notes are protected by the password and a device key with confirmed resistance to restoring a deleted key.")
        KeyGuarantee.DeviceKey -> tr("Заметки защищены паролем и ключом устройства (Android Keystore). Аппаратная гарантия не подтверждена.",
            "Notes are protected by the password and a device key (Android Keystore). A hardware guarantee is not confirmed.")
        KeyGuarantee.PasswordOnly -> tr("Заметки защищены только паролем: ключа устройства на этой платформе нет.",
            "Notes are protected by the password only: this platform has no device key.")
    }, style = MaterialTheme.typography.bodySmall)
}


@Composable
private fun CreateNotes(session: AppSession, status: NotesStatus, busy: Boolean, onCreate: (String) -> Unit) {
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    DisposableEffect(Unit) { onDispose { password = ""; repeat = "" } }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionHero(Res.drawable.ic_safe, tr("Сейф", "Safe"), tr("Защищённые заметки и личная папка", "Protected notes and personal folder"),
            Modifier.sectionHeading())
        ConceptCard(padding = 16.dp) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(Res.drawable.ic_safe, size = 88.dp)
            Spacer(Modifier.width(12.dp))
            Text(tr("Создайте пароль сейфа", "Create the Safe password"), style = MaterialTheme.typography.titleLarge,
                fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
        }
        Text(tr("Один отдельный пароль для заметок и личной папки. Содержимое Сейфа скрыто из файлового менеджера, не уходит в облако и не синхронизируется. Забытый пароль не восстановить.",
            "One separate password for notes and the personal folder. The Safe is hidden from the file manager, never leaves for the cloud and is not synced. A forgotten password cannot be recovered."),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedTextField(password, { password = it }, Modifier.fillMaxWidth(), singleLine = true,
            label = { Text(tr("Пароль", "Password")) }, visualTransformation = peek("NotesScreen:8"), trailingIcon = { PeekToggle("NotesScreen:8") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
        val strength = PasswordTools(session.core, password, !busy) {
            password = it; repeat = ""
        }
        OutlinedTextField(repeat, { repeat = it }, Modifier.fillMaxWidth().fieldError(passwordMismatchText(), repeat.isNotEmpty() && repeat != password), singleLine = true,
            label = { Text(tr("Повторите пароль", "Repeat password")) }, visualTransformation = peek("NotesScreen:9"), trailingIcon = { PeekToggle("NotesScreen:9") },
            isError = repeat.isNotEmpty() && repeat != password, supportingText = { if (repeat.isNotEmpty() && repeat != password) Text(passwordMismatchText()) },
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
        GuaranteeText(status.guarantee)
        Text(tr("Биометрия по умолчанию выключена; её можно включить после входа в настройках заметок. Пароль надёжнее биометрии.",
            "Biometrics are off by default; enable them in notes settings after unlocking. A password is safer than biometrics."), style = MaterialTheme.typography.bodySmall)
        Button(enabled = !busy && password == repeat && strength != null && strength != PasswordStrength.Weak,
            modifier = Modifier.fillMaxWidth().height(52.dp), shape = RoundedCornerShape(26.dp),
            onClick = { val p = password; password = ""; repeat = ""; onCreate(p) }) { Text(tr("Создать сейф", "Create the Safe")) }
        }
    }
}

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun UnlockNotes(status: NotesStatus, busy: Boolean, onRefresh: () -> Unit, onUnlock: (String) -> Unit) {
    var password by remember { mutableStateOf("") }
    var wait by remember(status) { mutableStateOf(status.retryAfterMs) }
    LaunchedEffect(status) {
        while (wait > 0) { delay(minOf(wait, 1000L)); wait = (wait - 1000L).coerceAtLeast(0) }
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionHero(Res.drawable.ic_safe, tr("Сейф", "Safe"), tr("Защищённые заметки и личная папка", "Protected notes and personal folder"),
            Modifier.sectionHeading())
        ConceptCard(padding = 16.dp) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            BrandIcon(Res.drawable.ic_key, size = 64.dp)
            Spacer(Modifier.width(12.dp))
            Text(tr("Сейф заблокирован", "The Safe is locked"), style = MaterialTheme.typography.titleLarge,
                fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
        }
        OutlinedTextField(password, { password = it }, Modifier.fillMaxWidth(), singleLine = true,
            label = { Text(tr("Пароль", "Password")) }, visualTransformation = peek("NotesScreen:10"), trailingIcon = { PeekToggle("NotesScreen:10") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
        // The attempt counter is deliberately not shown; only the wait and the auto-wipe state.
        if (wait > 0) Text(tr("Следующая попытка через ${(wait + 999) / 1000} с.", "Next attempt in ${(wait + 999) / 1000} s."))
        if (status.autoWipe) Text(tr("Включено автостирание: после серии неверных паролей заметки будут уничтожены.",
            "Auto-wipe is on: after repeated wrong passwords the notes are destroyed."), color = MaterialTheme.colorScheme.error,
            style = MaterialTheme.typography.bodySmall)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(enabled = !busy && wait == 0L && password.isNotEmpty(), onClick = { val p = password; password = ""; onUnlock(p) }) {
                Text(tr("Открыть", "Unlock"))
            }
            if (wait > 0) TextButton(enabled = !busy, onClick = onRefresh) { Text(tr("Обновить", "Refresh")) }
        }
        }
    }
}

@OptIn(kotlin.time.ExperimentalTime::class)
private fun nowMs(): Long = kotlin.time.Clock.System.now().toEpochMilliseconds()

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun ColumnScope.Notebook(tabs: List<NoteTab>, dirty: Boolean, busy: Boolean,
    writeBlocked: Boolean, pickerAvailable: Boolean,
    onChange: (List<NoteTab>) -> Unit, onSave: () -> Unit, onLock: () -> Unit,
    onPick: (String) -> Unit, onPreview: (NoteAttachment) -> Unit, onRemoveAttachment: (String, String) -> Unit) {
    val editingBlocked = busy || writeBlocked
    var selected by remember { mutableStateOf(0) }
    var editing by remember { mutableStateOf<String?>(null) }
    var renaming by remember { mutableStateOf(false) }
    var deleteTab by remember { mutableStateOf(false) }
    val index = selected.coerceIn(0, (tabs.size - 1).coerceAtLeast(0))
    val newTabTitle = tr("Вкладка", "Tab")
    val scheme = MaterialTheme.colorScheme
    var tabMenu by remember { mutableStateOf(false) }
    if (editing == null) Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(if (dirty) tr("Есть несохранённые изменения", "Unsaved changes") else tr("Всё сохранено", "All saved"),
            Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = if (dirty) scheme.tertiary else scheme.onSurfaceVariant)
        FilledTonalButton(enabled = !editingBlocked && dirty, onClick = onSave) { Text(tr("Сохранить", "Save")) }
        IconButton(enabled = !busy, onClick = onLock, modifier = Modifier.actionDescription(
            tr("Заблокировать (несохранённое пропадёт)", "Lock (unsaved changes are lost)"))) { AppIcon(AppSymbol.Vault) }
    }
    if (editing == null) LazyRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        itemsIndexed(tabs, key = { _, tab -> tab.id }) { i, tab ->
            FilterChip(selected = i == index, onClick = { selected = i; editing = null },
                label = { Text(displayName(tab.title), maxLines = 1, overflow = TextOverflow.Ellipsis) })
        }
        item(key = "+") {
            AssistChip(enabled = !editingBlocked, onClick = {
                onChange(tabs + NoteTab(randomRequestId(), "$newTabTitle ${tabs.size + 1}", emptyList())); selected = tabs.size; editing = null
            }, label = { Text(tr("Вкладка", "Tab")) }, leadingIcon = { AppIcon(AppSymbol.Add, Modifier.size(18.dp)) })
        }
        if (tabs.isNotEmpty()) item(key = "menu") { Box {
            IconButton(onClick = { tabMenu = true }, modifier = Modifier.actionDescription(tr("Действия с вкладкой", "Tab actions"))) { AppIcon(AppSymbol.More) }
            DropdownMenu(tabMenu, { tabMenu = false }) {
                DropdownMenuItem(text = { Text(tr("Переименовать вкладку", "Rename tab")) }, leadingIcon = { AppIcon(AppSymbol.Edit) },
                    enabled = !editingBlocked, onClick = { tabMenu = false; renaming = true })
                DropdownMenuItem(text = { Text(tr("Удалить вкладку", "Delete tab"), color = scheme.error) }, leadingIcon = { AppIcon(AppSymbol.Trash) },
                    enabled = !editingBlocked, onClick = { tabMenu = false; deleteTab = true })
            }
        } }
    }
    val tab = tabs.getOrNull(index)
    if (tab == null) { Text(tr("Создайте первую вкладку кнопкой «+ Вкладка».", "Create the first tab with \"+ Tab\"."), color = scheme.onSurfaceVariant); return }
    val note = tab.notes.firstOrNull { it.id == editing }
    if (note != null) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        fun update(n: Note) = onChange(tabs.mapIndexed { i, t -> if (i == index) t.copy(notes = t.notes.map { if (it.id == n.id) n else it }) else t })
        Row(verticalAlignment = Alignment.CenterVertically) {
            // Back saves first: leaving the editor never silently keeps edits only in memory.
            IconButton(onClick = { if (dirty && !editingBlocked) onSave(); editing = null },
                modifier = Modifier.actionDescription(tr("Сохранить и вернуться", "Save and go back"))) { AppIcon(AppSymbol.Back) }
            Column(Modifier.weight(1f)) {
                Text(displayName(tab.title), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                Text(if (dirty) tr("Не сохранено", "Not saved") else tr("Сохранено", "Saved"), style = MaterialTheme.typography.titleSmall,
                    color = if (dirty) scheme.tertiary else scheme.onSurface)
            }
            FilledTonalIconButton(enabled = !editingBlocked && dirty, onClick = onSave,
                modifier = Modifier.actionDescription(tr("Сохранить", "Save"))) { AppIcon(AppSymbol.Save) }
            IconButton(enabled = !editingBlocked, onClick = {
                onChange(tabs.mapIndexed { i, t -> if (i == index) t.copy(notes = t.notes.filterNot { it.id == note.id }) else t }); editing = null
            }, modifier = Modifier.actionDescription(tr("Удалить заметку", "Delete note"))) { AppIcon(AppSymbol.Trash) }
        }
        SecretInput { OutlinedTextField(note.title, { update(note.copy(title = it.take(256), modifiedMs = nowMs())) }, Modifier.fillMaxWidth(),
            enabled = !editingBlocked, singleLine = true, textStyle = MaterialTheme.typography.titleLarge,
            placeholder = { Text(tr("Заголовок", "Title")) }, shape = MaterialTheme.shapes.large) }
        SecretInput { OutlinedTextField(note.body, { update(note.copy(body = it, modifiedMs = nowMs())) }, Modifier.fillMaxWidth().heightIn(min = 220.dp, max = 420.dp),
            enabled = !editingBlocked, placeholder = { Text(tr("Текст заметки", "Note text")) }, shape = MaterialTheme.shapes.large) }
        SectionLabel(tr("Вложения (${note.attachments.size}/32)", "Attachments (${note.attachments.size}/32)"))
        note.attachments.forEachIndexed { i, attachment ->
            GroupedRow(displayName(attachment.name), groupPosition(i, note.attachments.size),
                subtitle = tr("${attachment.size} байт", "${attachment.size} bytes"), enabled = !busy,
                leading = { BrandIcon(Res.drawable.ic_documents, size = 52.dp) },
                trailing = { IconButton(enabled = !editingBlocked, onClick = { onRemoveAttachment(note.id, attachment.id) },
                    modifier = Modifier.actionDescription(tr("Удалить вложение: ", "Remove attachment: ") + displayName(attachment.name))) { AppIcon(AppSymbol.Close) } },
                onClick = { onPreview(attachment) })
        }
        Text(attachmentLimitText(), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        OutlinedButton(enabled = !editingBlocked && pickerAvailable && note.attachments.size < 32, onClick = { onPick(note.id) }) {
            AppIcon(AppSymbol.Add); Spacer(Modifier.width(8.dp)); Text(tr("Прикрепить файл", "Attach file"))
        }
        }
    } else {
        Button(enabled = !editingBlocked, modifier = Modifier.fillMaxWidth().height(52.dp), onClick = {
            val created = Note(randomRequestId(), "", "", nowMs())
            onChange(tabs.mapIndexed { i, t -> if (i == index) t.copy(notes = listOf(created) + t.notes) else t }); editing = created.id
        }) { AppIcon(AppSymbol.Edit); Spacer(Modifier.width(8.dp)); Text(tr("Новая заметка", "New note")) }
        if (tab.notes.isEmpty()) Column(Modifier.fillMaxWidth().padding(vertical = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            BrandIcon(Res.drawable.ic_safe, size = 100.dp)
            Text(tr("Во вкладке пока нет заметок", "No notes in this tab yet"), Modifier.padding(top = 12.dp), color = scheme.onSurfaceVariant)
        }
        tab.notes.forEachIndexed { i, n ->
            GroupedRow(displayName(n.title.ifBlank { tr("Без заголовка", "Untitled") }), groupPosition(i, tab.notes.size),
                Modifier.actionDescription(tr("Открыть заметку", "Open note")),
                subtitle = (displayName(n.body.lineSequence().firstOrNull().orEmpty().take(120)) +
                    if (n.attachments.isNotEmpty()) tr(" · вложений: ${n.attachments.size}", " · attachments: ${n.attachments.size}") else "").ifBlank { null },
                onClick = { editing = n.id })
        }
    }
    if (renaming) {
        var title by remember { mutableStateOf(tab.title) }
        AlertDialog(onDismissRequest = { renaming = false },
            title = { Text(tr("Имя вкладки", "Tab name")) },
            text = { SecretInput { OutlinedTextField(title, { title = it.take(128) }, singleLine = true, label = { Text(tr("Имя вкладки", "Tab name")) }) } },
            confirmButton = { TextButton(enabled = title.isNotBlank(), onClick = {
                onChange(tabs.mapIndexed { i, t -> if (i == index) t.copy(title = title) else t }); renaming = false
            }) { Text(tr("Сохранить", "Save")) } },
            dismissButton = { TextButton(onClick = { renaming = false }) { Text(tr("Отмена", "Cancel")) } })
    }
    if (deleteTab) AlertDialog(onDismissRequest = { deleteTab = false },
        title = { Text(tr("Удалить вкладку «${displayName(tab.title)}»?", "Delete tab \"${displayName(tab.title)}\"?")) },
        text = { Text(tr("Все заметки вкладки (${tab.notes.size}) будут удалены после сохранения.",
            "All notes of the tab (${tab.notes.size}) are deleted once you save.")) },
        confirmButton = { TextButton(onClick = {
            onChange(tabs.filterIndexed { i, _ -> i != index }); selected = 0; editing = null; deleteTab = false
        }) { Text(tr("Удалить", "Delete")) } },
        dismissButton = { TextButton(onClick = { deleteTab = false }) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun NotesSettings(session: AppSession, status: NotesStatus, busy: Boolean, biometricAvailable: Boolean,
    onQuick: (Boolean) -> Unit, onAutoWipe: (Boolean) -> Unit,
    onDuress: (String?) -> Unit, onChangePassword: (String, String) -> Unit, onClear: () -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    var changing by remember { mutableStateOf(false) }
    var clearing by remember { mutableStateOf(false) }
    var enablingQuick by remember { mutableStateOf(false) }
    val scheme = MaterialTheme.colorScheme
    val quickAllowed = !busy && (status.quickEnabled || status.quickAvailable && biometricAvailable)
    SectionLabel(tr("Безопасность Сейфа", "Safe security"))
    GroupedRow(tr("Вход по биометрии", "Biometric unlock"), GroupPosition.First,
        subtitle = if (!biometricAvailable || !status.quickAvailable) tr("Нужны Android 11+ и сильная биометрия", "Needs Android 11+ and strong biometrics")
            else tr("Отпечаток или лицо вместо пароля", "Fingerprint or face instead of the password"),
        enabled = quickAllowed,
        leading = { BrandIcon(Res.drawable.ic_fingerprint, size = 52.dp) },
        trailing = { Switch(checked = status.quickEnabled, enabled = quickAllowed,
            onCheckedChange = { enable -> if (enable) enablingQuick = true else onQuick(false) }) },
        onClick = { if (status.quickEnabled) onQuick(false) else enablingQuick = true })
    Spacer(Modifier.height(2.dp))
    GroupedRow(tr("Сменить пароль", "Change password"), GroupPosition.Middle, enabled = !busy,
        leading = { BrandIcon(Res.drawable.ic_key, size = 52.dp) },
        trailing = { AppIcon(AppSymbol.Forward) }, onClick = { changing = true })
    Spacer(Modifier.height(2.dp))
    GroupedRow(tr("Clear — стереть Сейф", "Clear — erase the Safe"), GroupPosition.Middle, enabled = !busy,
        subtitle = tr("Заметки и личная папка уничтожаются вместе с ключами", "Notes and the personal folder are destroyed with their keys"),
        leading = { BrandIcon(Res.drawable.ic_trash_full, size = 52.dp) },
        onClick = { clearing = true })
    Spacer(Modifier.height(2.dp))
    GroupedRow(tr("Дополнительно", "More"), GroupPosition.Last,
        subtitle = tr("Автостирание, пароль под принуждением", "Auto-wipe, duress password"),
        leading = { BrandIcon(Res.drawable.ic_settings, size = 52.dp) },
        trailing = { AppIcon(if (expanded) AppSymbol.Up else AppSymbol.Forward) }, onClick = { expanded = !expanded })
    if (enablingQuick) AlertDialog(onDismissRequest = { enablingQuick = false },
        title = { Text(tr("Включить быстрый вход?", "Enable quick unlock?")) },
        text = { Text(tr("Биометрия — дополнительный вход в заметки без пароля. Пароль надёжнее.",
            "Biometrics add password-free access to notes. A password is stronger.")) },
        confirmButton = { TextButton(enabled = !busy, onClick = { enablingQuick = false; onQuick(true) }) { Text(tr("Подтвердить биометрией", "Confirm with biometrics")) } },
        dismissButton = { TextButton(onClick = { enablingQuick = false }) { Text(tr("Отмена", "Cancel")) } })
    if (expanded) Column(Modifier.padding(top = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        GuaranteeText(status.guarantee)
    LabeledSwitch(tr("Автостирание после ${status.wipeAfter} неверных паролей", "Auto-wipe after ${status.wipeAfter} wrong passwords"),
        checked = status.autoWipe, enabled = !busy, onChange = onAutoWipe)
    NotesDuressSettings(session, !busy, onDuress)
    Text(tr("Защищает от подбора, но опечатки тоже считаются.", "Protects against guessing, but typos count too."),
        style = MaterialTheme.typography.bodySmall)
    }
    if (changing) {
        var old by remember { mutableStateOf("") }
        var new by remember { mutableStateOf("") }
        val passwordAssessment = rememberPasswordAssessment(session.core, new)
        var repeated by remember { mutableStateOf("") }
        DisposableEffect(Unit) { onDispose { old = ""; new = ""; repeated = "" } }
        AlertDialog(onDismissRequest = { old = ""; new = ""; repeated = ""; changing = false },
            title = { Text(tr("Сменить пароль заметок", "Change notes password")) },
            text = {
                Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(old, { old = it }, singleLine = true, label = { Text(tr("Текущий пароль", "Current password")) },
                        visualTransformation = peek("NotesScreen:11"), trailingIcon = { PeekToggle("NotesScreen:11") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                    OutlinedTextField(new, { new = it }, singleLine = true, label = { Text(tr("Новый пароль", "New password")) },
                        visualTransformation = peek("NotesScreen:12"), trailingIcon = { PeekToggle("NotesScreen:12") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                    PasswordTools(session.core, new, !busy, assessment = passwordAssessment) { new = it; repeated = "" }
                    OutlinedTextField(repeated, { if (it.length <= 4096) repeated = it }, singleLine = true,
                        label = { Text(tr("Повторите новый пароль", "Repeat new password")) },
                        visualTransformation = peek("NotesScreen:13"), trailingIcon = { PeekToggle("NotesScreen:13") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
                        isError = repeated.isNotEmpty() && repeated != new,
                        supportingText = { if (repeated.isNotEmpty() && repeated != new) Text(passwordMismatchText()) })
                }
            },
            confirmButton = { TextButton(enabled = !busy && old.isNotEmpty() && passwordAssessment.acceptsPassword() && new == repeated && old != new, onClick = {
                val o = old; val n = new; old = ""; new = ""; repeated = ""; changing = false; onChangePassword(o, n)
            }) { Text(tr("Сменить", "Change")) } },
            dismissButton = { TextButton(onClick = { changing = false }) { Text(tr("Отмена", "Cancel")) } })
    }
    if (clearing) {
        var confirmed by remember { mutableStateOf(false) }
        AlertDialog(onDismissRequest = { clearing = false },
            title = { Text(tr("Стереть заметки и личную папку?", "Erase notes and the personal folder?")) },
            text = {
                Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(tr("Ключи Сейфа будут уничтожены — заметки и личная папка пропадут навсегда. Копии снаружи не удаляются.",
                        "Safe keys are destroyed — notes and the personal folder are gone for good. Copies outside are not deleted."))
                    Text(when (status.guarantee) {
                        KeyGuarantee.PasswordOnly -> tr("На этой платформе нет ключа устройства: старые копии файлов (резервные копии) остаются защищены только паролем.",
                            "This platform has no device key: old copies of the files (backups) remain protected by the password only.")
                        KeyGuarantee.DeviceKey -> tr("Ключ устройства удаляется. Стирание резервных копий не гарантируется.",
                            "The device key is deleted. Erasing backups is not guaranteed.")
                        KeyGuarantee.HardwareKey -> tr("Ключ удаляется — даже сохранённые где-то копии файлов перестанут расшифровываться.",
                            "The key is deleted — even saved copies of the files stop decrypting.")
                    }, style = MaterialTheme.typography.bodySmall)
                    LabeledCheckbox(tr("Понимаю, заметки и личная папка будут уничтожены", "I understand notes and the personal folder will be destroyed"),
                        checked = confirmed, onChange = { confirmed = it })
                }
            },
            confirmButton = { TextButton(enabled = confirmed && !busy, onClick = { clearing = false; onClear() }) {
                Text(tr("Стереть", "Erase"), color = MaterialTheme.colorScheme.error)
            } },
            dismissButton = { TextButton(onClick = { clearing = false }) { Text(tr("Отмена", "Cancel")) } })
    }
}
