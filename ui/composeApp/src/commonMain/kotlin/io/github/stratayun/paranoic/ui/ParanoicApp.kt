package io.github.stratayun.paranoic.ui

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.navigation.*
import io.github.stratayun.paranoic.ui.screens.files.FilesScreen
import io.github.stratayun.paranoic.ui.screens.home.HomeScreen
import io.github.stratayun.paranoic.ui.screens.home.ModulesScreen
import io.github.stratayun.paranoic.ui.screens.files.FileWorkspaceScreen
import io.github.stratayun.paranoic.ui.screens.encryption.EncryptionScreen
import io.github.stratayun.paranoic.ui.screens.notes.NotesScreen
import io.github.stratayun.paranoic.ui.screens.documents.FileViewer
import io.github.stratayun.paranoic.ui.screens.operations.OperationsScreen
import io.github.stratayun.paranoic.ui.screens.settings.AppearanceScreen
import io.github.stratayun.paranoic.ui.screens.settings.VaultDefaultsSettings
import io.github.stratayun.paranoic.ui.screens.settings.DiagnosticsScreen
import io.github.stratayun.paranoic.ui.screens.settings.WelcomeFlow
import io.github.stratayun.paranoic.ui.screens.sync.SyncScreen
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.theme.ParanoicTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Platform composition roots inject one Core gateway, a preference store and a picker.
 * No native library loading, OS paths, passwords or file operations in this module.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ParanoicApp(core: CoreGateway, store: AppearanceStore,
    folderPicker: @Composable ((FolderPick) -> Unit) -> (() -> Unit),
    pngImage: @Composable (ByteArray, Modifier) -> Unit,
    mediaPlayer: @Composable (CoreGateway, Location, PreviewInfo, Modifier) -> Unit,
    vaultCatalogStore: VaultCatalogStore = remember { MemoryVaultCatalogStore() },
    placeCatalogStore: PlaceCatalogStore = remember { MemoryPlaceCatalogStore() },
    diagnosticEnvironment: DiagnosticEnvironment = NoDiagnosticEnvironment,
    securitySettings: @Composable () -> Unit = {},
    notesBiometric: NotesBiometric = NoNotesBiometric,
    notesAttachmentPicker: NotesAttachmentPicker = NoNotesAttachmentPicker,
    incomingDocuments: IncomingDocuments = NoIncomingDocuments,
    notesImage: @Composable (ByteArray, Modifier) -> Unit = pngImage,
    backHandler: @Composable (Boolean, () -> Unit) -> Unit = { _, _ -> },
    onLanguageChange: (UiLanguage) -> Unit = {},
    sessionLifecycle: @Composable (AppSession) -> Unit = { session ->
        LaunchedEffect(session) { session.resume() }
    }) {
    val scope = rememberCoroutineScope()
    val session = remember(core, scope, vaultCatalogStore, placeCatalogStore) {
        AppSession(core, scope, catalogStore = vaultCatalogStore, placesStore = placeCatalogStore)
    }
    val placesDrawer = rememberDrawerState(DrawerValue.Closed)
    var settings by remember { mutableStateOf(AppearanceSettings()) }
    var loaded by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf<Boolean?>(null) }
    var destination by remember { mutableStateOf(Destination.Home) }
    var pinPicking by remember { mutableStateOf(false) }
    var pickerFailed by remember { mutableStateOf(false) }
    val vaultFolderPicker = remember(session) { VaultFolderPickerFlow() }
    var vaultFolderReturn by remember(session) { mutableStateOf<Pair<Long, VaultFolderReturn>?>(null) }
    LaunchedEffect(session.secretsEpoch) {
        if (vaultFolderReturn?.first != session.secretsEpoch) vaultFolderReturn = null
        placesDrawer.close()
    }
    var moreMenu by remember { mutableStateOf(false) }
    var openedFile by remember { mutableStateOf<FileItem?>(null) }
    val openedParent = remember(openedFile?.location) { session.files.folder }
    val documentExit = remember(core, openedFile?.location, destination, session.secretsEpoch) { DocumentExitGuard() }
    val leaveDocument: (() -> Unit) -> Unit = { action ->
        documentExit.request {
            val wasOpen = openedFile != null
            openedFile = null
            if (wasOpen) session.refresh()
            action()
        }
    }
    LaunchedEffect(session.files.folder) {
        // Security lifecycle invalidates the viewer even when edits exist.
        if (session.files.folder == null) { documentExit.cancel(); openedFile = null }
    }
    LaunchedEffect(store) {
        try { settings = withContext(Dispatchers.Default) { store.read() } }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { saved = false }
        loaded = true
    }
    sessionLifecycle(session)
    SideEffect { if (loaded) onLanguageChange(settings.language) }
    val changeSettings: (AppearanceSettings) -> Unit = { next ->
        if (!saving) {
            settings = next
            onLanguageChange(next.language)
            saving = true
            scope.launch {
                try {
                    saved = withContext(Dispatchers.Default) { store.write(next) }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { saved = false }
                finally { saving = false }
            }
        }
    }
    val pickFolder = folderPicker { result ->
        when (result) {
            is FolderPick.Selected -> { session.attach(result.folder); destination = Destination.Files }
            FolderPick.Cancelled -> Unit
            FolderPick.Failed -> pickerFailed = true
        }
    }
    val guardedPickFolder: () -> Unit = { leaveDocument { pickFolder() } }
    // Separate registration: after process recreation a late vault-picker result
    // cannot accidentally open Files or recreate a discarded password dialog.
    val pickVaultFolder = folderPicker { result ->
        val request = vaultFolderPicker.acceptSelection()
        if (request != null) {
            fun deliver(folder: FolderNode? = null, fault: CoreFault? = null) {
                val returned = vaultFolderPicker.finish(request, folder, fault)
                if (returned != null && destination == Destination.Encryption)
                    vaultFolderReturn = session.secretsEpoch to returned
            }
            when (result) {
                is FolderPick.Selected -> session.attach(result.folder) { attached ->
                    when (attached) {
                        is CoreResult.Value -> deliver(FolderNode(attached.value.root, attached.value.name))
                        is CoreResult.Failure -> deliver(fault = attached.fault)
                    }
                }
                FolderPick.Cancelled -> deliver()
                FolderPick.Failed -> deliver(fault = CoreFault.Unavailable)
            }
        }
    }
    val openSpace: (Space) -> Unit = { space -> leaveDocument { session.openSpace(space); destination = Destination.Files } }
    val detachSpace: (Space) -> Unit = { space -> leaveDocument { session.detach(space) } }
    val connectServer: (String, String, String) -> Unit = { url, user, token ->
        leaveDocument { session.attachWebDav(url, user, token) }
    }
    backHandler(placesDrawer.isOpen || openedFile != null || session.categories.state != null || destination != Destination.Home ||
        (destination == Destination.Files && (session.archives.state.source != null || session.files.selected.isNotEmpty() || session.files.trail.size > 1))) {
        when {
            placesDrawer.isOpen -> scope.launch { placesDrawer.close() }
            destination == Destination.Files && session.archives.state.source != null -> if (!session.writing) session.archives.close()
            openedFile != null -> leaveDocument {}
            destination == Destination.Files && session.categories.state != null -> session.categories.close()
            destination == Destination.Files && session.files.selected.isNotEmpty() -> session.clearSelection()
            destination == Destination.Files && session.files.trail.size > 1 -> session.breadcrumb(session.files.trail.lastIndex - 1)
            destination != Destination.Home -> leaveDocument { destination = Destination.Home }
        }
    }
    val vaultContent: @Composable () -> Unit = {
        key(session.secretsEpoch) {
            EncryptionScreen(session, exitGuard = documentExit, incoming = incomingDocuments, secureImage = notesImage, mediaPlayer = mediaPlayer,
                folderReturn = vaultFolderReturn?.takeIf { it.first == session.secretsEpoch }?.second,
                onFolderReturnConsumed = { vaultFolderReturn = null }, backHandler = backHandler,
                onConnectFolder = { request -> leaveDocument { if (vaultFolderPicker.begin(request)) pickVaultFolder() } }) {
                destination = Destination.Files
            }
        }
    }
    LaunchedEffect(destination) { if (destination == Destination.Files) session.refresh() }
    val peekStates = remember { PeekStates() }
    CompositionLocalProvider(LocalUiLanguage provides settings.language, LocalErrorCore provides core, LocalPeekStates provides peekStates) {
        ParanoicTheme(settings) {
            if (!loaded) {
                Surface(Modifier.fillMaxSize()) {
                    Box(contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                }
                return@ParanoicTheme
            }
            if (!settings.welcomeCompleted) {
                WelcomeFlow(core, settings.language, session.secretsEpoch, documentExit.busy || session.writing, saving, saved,
                    onLanguage = { changeSettings(settings.copy(language = it)) }, onBusy = { documentExit.busy = it },
                    onFinish = {
                        if (!saving && !documentExit.busy && !session.writing) {
                            session.pause(); session.resume(); destination = Destination.Home
                            changeSettings(settings.copy(welcomeCompleted = true))
                        }
                    }, securitySettings = securitySettings, vaultContent = vaultContent, backHandler = backHandler)
                return@ParanoicTheme
            }
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val wide = maxWidth >= 840.dp
                LaunchedEffect(wide) { if (wide) placesDrawer.close() }
                // Read here: nested layout scopes share the DSL marker, so
                // maxHeight is not reachable implicitly inside Column below.
                val screenHeight = maxHeight
                val fullScreenFile = openedFile
                if (fullScreenFile != null && destination == Destination.Files) {
                    FileViewer(core, fullScreenFile, { leaveDocument {} }, pngImage, mediaPlayer,
                        textParent = openedParent, exitGuard = documentExit)
                    return@BoxWithConstraints
                }
                val drawerFooter: @Composable (Boolean) -> Unit = { closeAfter ->
                    fun go(target: Destination) {
                        if (closeAfter) scope.launch { placesDrawer.close() }
                        if (destination != target) leaveDocument { destination = target }
                    }
                    Spacer(Modifier.height(8.dp))
                    DrawerFooter(enabled = !documentExit.busy,
                        showFiles = session.files.spaces.userPlaces().none { !it.cloud && !(it.localContainer && !it.detachable) },
                        runningTasks = session.jobs.count { it.phase == JobPhase.Queued || it.phase == JobPhase.Running },
                        onSafe = { go(Destination.Notes) }, onVaults = { go(Destination.Encryption) }, onFiles = { go(Destination.Files) },
                        current = destination,
                        onSync = { go(Destination.Sync) }, onDiagnostics = { go(Destination.Diagnostics) },
                        onTasks = { go(Destination.Operations) }, onSettings = { go(Destination.Settings) })
                }
                ModalNavigationDrawer(drawerState = placesDrawer, gesturesEnabled = !wide && !documentExit.busy,
                    drawerContent = {
                        if (!wide) ModalDrawerSheet(Modifier.widthIn(max = 380.dp),
                            drawerShape = RoundedCornerShape(topEnd = 32.dp, bottomEnd = 32.dp),
                            drawerContainerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                            Column(Modifier.fillMaxHeight().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
                                DrawerHeader { scope.launch { placesDrawer.close() } }
                                key(session.secretsEpoch) {
                                SpacesPanel(session.files.spaces, session.places, core.connected, session.writing || documentExit.busy,
                                    onConnect = { scope.launch { placesDrawer.close() }; guardedPickFolder() },
                                    onOpen = { space -> openSpace(space); scope.launch { placesDrawer.close() } },
                                    onDetach = detachSpace, onConnectServer = connectServer,
                                    onOpenPath = { space, path -> leaveDocument { session.openPath(space, path); destination = Destination.Files }; scope.launch { placesDrawer.close() } },
                                    onOpenCategory = { space, category -> leaveDocument { session.categories.open(space, category); destination = Destination.Files }; scope.launch { placesDrawer.close() } },
                                    onOpenTrash = { space -> session.trashView = space; scope.launch { placesDrawer.close() } }, trashFilled = session.trashFilled,
                                    onOpenCleanup = { space -> session.cleanupView = space; scope.launch { placesDrawer.close() } },
                                    bookmarkCore = core)
                                }
                                drawerFooter(true)
                            }
                        }
                    }) {
                val searchPlace = session.files.spaces.userPlaces().firstOrNull { it.detachable && !it.cloud }
                val runningTasks = session.jobs.count { it.phase == JobPhase.Queued || it.phase == JobPhase.Running }
                Scaffold(
                    bottomBar = {
                        // Owner's phone mockup (27.11): bottom tabs; wide windows keep the side panel.
                        if (!wide) NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                            bottomTabs.forEach { tab ->
                                NavigationBarItem(selected = destination == tab, enabled = !documentExit.busy,
                                    onClick = { if (destination != tab) leaveDocument { destination = tab } },
                                    icon = { AppIcon(when (tab) {
                                        Destination.Home -> AppSymbol.Home
                                        Destination.Files -> AppSymbol.Folder
                                        Destination.Modules -> AppSymbol.ViewGrid
                                        Destination.Sync -> AppSymbol.Sync
                                        else -> AppSymbol.Settings
                                    }) },
                                    label = { Text(if (tab == Destination.Sync) tr("Синхронизация", "Sync") else tab.label(),
                                        style = MaterialTheme.typography.labelSmall, maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis) })
                            }
                        }
                    },
                    topBar = {
                        TopAppBar(title = {
                                // Concept: the brand in the top bar; the page itself carries its large title.
                                if (destination in setOf(Destination.Home, Destination.Modules, Destination.Sync, Destination.Settings))
                                    Text("Paranoic", Modifier.sectionHeading(), maxLines = 1,
                                    fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
                                else Text(destination.label(), Modifier.sectionHeading(), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            navigationIcon = {
                                if (destination !in mainDestinations) IconButton(onClick = { leaveDocument { destination = Destination.Home } },
                                    modifier = Modifier.actionDescription(tr("Назад", "Back"))) { AppIcon(AppSymbol.Back) }
                                else if (!wide) IconButton(onClick = { scope.launch { placesDrawer.open() } },
                                    modifier = Modifier.actionDescription(tr("Меню и места", "Menu and places"))) { AppIcon(AppSymbol.Menu) }
                            },
                            actions = {
                                if (incomingDocuments.documents.isNotEmpty()) TextButton(onClick = {
                                    leaveDocument { destination = Destination.Notes }
                                }) { Text(tr("Входящие (${incomingDocuments.documents.size})", "Incoming (${incomingDocuments.documents.size})")) }
                                if (destination == Destination.Home) {
                                    if (searchPlace != null) IconButton(onClick = { leaveDocument { session.categories.open(searchPlace, FileCategory.All); destination = Destination.Files } },
                                        modifier = Modifier.actionDescription(tr("Поиск в Paranoic", "Search in Paranoic"))) { AppIcon(AppSymbol.Search) }
                                    IconButton(onClick = { leaveDocument { destination = Destination.Operations } },
                                        modifier = Modifier.actionDescription(tr("Задачи и события", "Tasks and events"))) {
                                        if (runningTasks > 0) BadgedBox(badge = { Badge { Text("$runningTasks") } }) { AppIcon(AppSymbol.Tasks) }
                                        else AppIcon(AppSymbol.Tasks)
                                    }
                                }
                                if (destination == Destination.Files && searchPlace != null && session.categories.state == null)
                                    IconButton(onClick = { session.categories.open(searchPlace, FileCategory.All) },
                                        modifier = Modifier.actionDescription(tr("Поиск по всем файлам", "Search all files"))) { AppIcon(AppSymbol.Search) }
                                // Tasks appear here only while something runs; everything else lives in the ☰ menu.
                                val running = runningTasks
                                if (running > 0 && destination != Destination.Home) IconButton(onClick = { leaveDocument { destination = Destination.Operations } },
                                    modifier = Modifier.actionDescription(tr("Задачи: $running", "Tasks: $running"))) {
                                    BadgedBox(badge = { Badge { Text("$running") } }) { AppIcon(AppSymbol.Tasks) }
                                }
                            })
                    },
                ) { insets ->
                    Column(Modifier.fillMaxSize().padding(insets)) {
                        Column(Modifier.fillMaxWidth().heightIn(max = screenHeight * 0.30f).verticalScroll(rememberScrollState())) {
                        if (!core.connected) FaultNotice(CoreFault.Disconnected, Modifier.fillMaxWidth().padding(12.dp))
                        if (destination != Destination.Notes) ProtectedWriteNotice(session)
                        incomingDocuments.fault?.let { fault ->
                            FaultNotice(fault, message = tr("Не удалось принять файл. Нужны временный доступ и точный размер; поделитесь файлом заново.",
                                "The file could not be accepted. Temporary read access and an exact size are required; share the file again."))
                            TextButton(onClick = incomingDocuments::clear) { Text(tr("Закрыть входящие", "Dismiss incoming files")) }
                        }
                        session.writeFault?.let { fault ->
                            FaultNotice(fault, Modifier.padding(12.dp))
                        }
                        session.directWrite?.let { operation ->
                            // Success notices fade on their own; failures and unknown outcomes stay until dismissed.
                            LaunchedEffect(operation) {
                                if (operation.phase == DirectWritePhase.Done) { kotlinx.coroutines.delay(4000); session.dismissDirectWrite() }
                            }
                            Surface(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), shape = MaterialTheme.shapes.large,
                                color = when (operation.phase) {
                                    DirectWritePhase.Failed, DirectWritePhase.Unknown -> MaterialTheme.colorScheme.errorContainer
                                    DirectWritePhase.Done -> MaterialTheme.colorScheme.secondaryContainer
                                    DirectWritePhase.Running -> MaterialTheme.colorScheme.surfaceContainerHigh
                                }) { Column(Modifier.padding(vertical = 4.dp)) {
                            Text(when (operation.phase) {
                                DirectWritePhase.Running -> tr("Выполняется… Исходники не меняются.", "Working… Sources stay unchanged.")
                                DirectWritePhase.Done -> tr("Готово: новый файл создан.", "Done: new file created.")
                                DirectWritePhase.Failed -> if (operation.kind == DirectWriteKind.DecryptFile && session.writeFault == CoreFault.WrongPassword)
                                    tr("Неверный пароль или файл повреждён.", "Wrong password or damaged file.")
                                    else tr("Не завершено — см. ошибку выше.", "Not completed — see the error above.")
                                DirectWritePhase.Unknown -> tr("Результат неизвестен. Проверьте папку перед новой записью; повтора не будет.",
                                    "Outcome unknown. Check the folder before writing again; nothing is retried.")
                            }, Modifier.padding(horizontal = 12.dp))
                            session.cryptoBatch?.takeIf { operation.kind == DirectWriteKind.EncryptFile || operation.kind == DirectWriteKind.DecryptFile }?.let { (done, total) ->
                                Text(tr("Файлов: $done из $total", "Files: $done of $total"), Modifier.padding(horizontal = 12.dp),
                                    style = MaterialTheme.typography.labelMedium)
                            }
                            if (operation.phase == DirectWritePhase.Running) {
                                LinearProgressIndicator(Modifier.fillMaxWidth())
                                if (operation.requestId != null) TextButton(onClick = session::cancelDirectArchive,
                                    enabled = !operation.cancelling) {
                                    Text(if (operation.cancelling) tr("Отмена запрошена…", "Cancellation requested…") else tr("Отменить", "Cancel"))
                                }
                            } else TextButton(onClick = session::dismissDirectWrite) {
                                Text(if (operation.phase == DirectWritePhase.Unknown) tr("Проверил(а), закрыть", "Inspected, dismiss") else tr("Закрыть", "Dismiss"))
                            }
                            } }
                        }
                        session.uncertain?.let { pending ->
                            Text(when (pending.notInJournal) {
                                true -> tr("Запроса нет в журнале: он не был принят. Можно отправить его ещё раз (с тем же ID повтора не будет).",
                                    "The request is not in the journal: it was not accepted. It can be sent again (the same ID cannot run twice).")
                                else -> tr("Запрос ${pending.requestId.take(8)}…: ответ потерян. Проверьте журнал по ID.",
                                    "Request ${pending.requestId.take(8)}…: reply lost. Check the journal by ID.")
                            }, Modifier.padding(horizontal = 12.dp))
                            Row(Modifier.padding(horizontal = 12.dp)) {
                                TextButton(onClick = session::checkUncertain, enabled = core.connected && !session.writing) {
                                    Text(tr("Проверить в журнале", "Check journal"))
                                }
                                if (pending.notInJournal == true) TextButton(onClick = session::resendUncertain,
                                    enabled = core.connected && !session.writing) { Text(tr("Отправить ещё раз", "Send again")) }
                            }
                        }
                        session.uncertainAttention?.let { pending ->
                            Text(tr("Задание ${pending.jobId.take(8)}…: ответ неизвестен. Обновите состояние; повтора не было.",
                                "Job ${pending.jobId.take(8)}…: reply unknown. Refresh the state; nothing was retried."),
                                Modifier.padding(horizontal = 12.dp))
                            TextButton(onClick = session::checkAttentionDecision,
                                enabled = core.connected && !session.writing && !session.attention.loading) {
                                Text(tr("Проверить состояние решения", "Check decision state"))
                            }
                        }
                        // The operation banner's own "Dismiss" also clears its error: no second button.
                        if ((session.writeFault != null && session.directWrite == null) || session.uncertain != null)
                            TextButton(onClick = session::dismissWriteFault) { Text(tr("Закрыть сообщение", "Dismiss message")) }
                        }
                        Row(Modifier.weight(1f)) {
                            if (wide) {
                                Column(Modifier.width(320.dp).fillMaxHeight().verticalScroll(rememberScrollState())) {
                                    key(session.secretsEpoch) {
                                    SpacesPanel(session.files.spaces, session.places, core.connected, session.writing || documentExit.busy, guardedPickFolder, openSpace, detachSpace,
                                        onConnectServer = connectServer,
                                        onOpenPath = { space, path -> leaveDocument { session.openPath(space, path); destination = Destination.Files } },
                                        onOpenCategory = { space, category -> leaveDocument { session.categories.open(space, category); destination = Destination.Files } },
                                        onOpenTrash = { space -> session.trashView = space }, trashFilled = session.trashFilled,
                                        onOpenCleanup = { space -> session.cleanupView = space },
                                        bookmarkCore = core)
                                    }
                                    drawerFooter(false)
                                }
                            }
                            Box(Modifier.weight(1f).fillMaxHeight()) {
                                when (destination) {
                                    Destination.Home -> key(session.secretsEpoch) {
                                        HomeScreen(session.files.spaces, session.places,
                                            onFiles = { destination = Destination.Files },
                                            onVaults = { destination = Destination.Encryption },
                                            onSafe = { destination = Destination.Notes },
                                            onDiagnostics = { destination = Destination.Diagnostics },
                                            onOpenSpace = openSpace,
                                            onOpenPath = { space, path -> leaveDocument { session.openPath(space, path); destination = Destination.Files } },
                                            onShowPlaces = { if (!wide) scope.launch { placesDrawer.open() } },
                                            onPinFolder = { pinPicking = true })
                                    }
                                    Destination.Modules -> ModulesScreen(
                                        onVaults = { destination = Destination.Encryption },
                                        onSafe = { destination = Destination.Notes },
                                        onDiagnostics = { destination = Destination.Diagnostics },
                                        onTasks = { destination = Destination.Operations },
                                        onSync = { destination = Destination.Sync },
                                        onSettings = { destination = Destination.Settings },
                                        onFiles = { destination = Destination.Files },
                                        onCleanup = searchPlace?.let { place -> { session.cleanupView = place } },
                                        onTrash = searchPlace?.let { place -> { session.trashView = place } },
                                        onSearch = searchPlace?.let { place -> { session.categories.open(place, FileCategory.All); destination = Destination.Files } })
                                    Destination.Files -> FileWorkspaceScreen(session, settings.fileView, { openedFile = it }, pngImage,
                                        onOpenPlaces = if (wide) null else ({ scope.launch { placesDrawer.open() }; Unit }),
                                        onNewVault = { leaveDocument { destination = Destination.Encryption } },
                                        onSendToSafe = { items -> session.safeInbox = items; leaveDocument { destination = Destination.Notes } })
                                    Destination.Operations -> OperationsScreen(session) { location ->
                                        if (session.openAttentionFolder(location)) {
                                            openedFile = null
                                            destination = Destination.Files
                                        }
                                    }
                                    Destination.Settings -> AppearanceScreen(settings, saving, saved, changeSettings,
                                        securitySettings = { securitySettings(); VaultDefaultsSettings(session.vaultCatalog) }, backHandler = backHandler,
                                        diagnosticsSettings = { TextButton(onClick = { destination = Destination.Diagnostics }) { Text(tr("Открыть журнал и отчёт", "Open journal and report")) } },
                                        onRestartWelcome = { destination = Destination.Files; changeSettings(settings.copy(welcomeCompleted = false)) })
                                    Destination.Diagnostics -> io.github.stratayun.paranoic.ui.screens.settings.DiagnosticsAndTasks(session.jobs.count { it.phase == JobPhase.Queued || it.phase == JobPhase.Running },
                                        diagnostics = { DiagnosticsScreen(session, diagnosticEnvironment) { destination = Destination.Files } },
                                        tasks = { OperationsScreen(session) { location ->
                                            if (session.openAttentionFolder(location)) { openedFile = null; destination = Destination.Files }
                                        } })
                                    Destination.Encryption -> vaultContent()
                                    Destination.Notes -> key(session.secretsEpoch) {
                                        NotesScreen(session, notesBiometric, notesAttachmentPicker, notesImage,
                                            onDiagnosticsCleared = { clearDiagnosticRecords(core, diagnosticEnvironment) }, exitGuard = documentExit,
                                            incoming = incomingDocuments, vaultMediaPlayer = mediaPlayer)
                                    }
                                    Destination.Sync -> key(session.secretsEpoch) {
                                        SyncScreen(core, onFiles = { destination = Destination.Files },
                                            onJobs = { destination = Destination.Operations })
                                    }
                                }
                            }
                        }
                    }
                }
            }
            }
            if (documentExit.pending != null) AlertDialog(onDismissRequest = documentExit::cancel,
                title = { Text(tr("Закрыть без сохранения?", "Discard unsaved edits?")) },
                text = { Text(if (documentExit.uncertain)
                    tr("Результат записи неизвестен — проверьте папку. Несохранённые правки будут потеряны.",
                        "Write outcome unknown — check the folder. Unsaved edits will be lost.")
                    else tr("Несохранённые правки будут потеряны.", "Unsaved edits will be lost.")) },
                confirmButton = { TextButton(onClick = documentExit::discard) { Text(tr("Продолжить", "Continue")) } },
                dismissButton = { TextButton(onClick = documentExit::cancel) { Text(tr("Остаться", "Stay")) } })
            session.cleanupView?.let { space ->
                io.github.stratayun.paranoic.ui.screens.files.CleanupScreen(session, space, onClose = { session.cleanupView = null })
            }
            session.trashView?.let { space ->
                io.github.stratayun.paranoic.ui.screens.files.TrashScreen(session, space, onClose = { session.trashView = null })
            }
            if (pinPicking) CoreFilePickerDialog(core, session.files.spaces, tr("Закрепить папку", "Pin a folder"), CorePickKind.Directory,
                onDismiss = { pinPicking = false }) { picked ->
                pinPicking = false
                picked.firstOrNull()?.takeIf { it.location.path.isNotEmpty() }?.let { folder ->
                    val label = folder.name.take(120)
                    if (validPlaceTitle(label)) session.places.pin(PlaceBookmark(folder.location.storageId, folder.location.path, label))
                }
            }
            if (pickerFailed) AlertDialog(onDismissRequest = { pickerFailed = false },
                title = { Text(tr("Выбор места недоступен", "Location selection unavailable")) },
                text = { Text(tr("Не удалось получить доступ к выбранному месту.", "Could not access the selected location.")) },
                confirmButton = { TextButton(onClick = { pickerFailed = false }) { Text(tr("Понятно", "OK")) } })
        }
    }
}
private fun Destination.symbol() = when (this) {
    Destination.Home -> AppSymbol.Home
    Destination.Modules -> AppSymbol.ViewGrid
    Destination.Files -> AppSymbol.Folder
    Destination.Encryption -> AppSymbol.Security
    Destination.Notes -> AppSymbol.Document
    Destination.Sync -> AppSymbol.Recovery
    Destination.Operations -> AppSymbol.Recovery
    Destination.Settings -> AppSymbol.Settings
    Destination.Diagnostics -> AppSymbol.Settings
}
