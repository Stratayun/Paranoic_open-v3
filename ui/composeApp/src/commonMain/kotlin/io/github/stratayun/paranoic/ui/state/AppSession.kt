package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Owns presentation state only. Native storage, permissions and the durable queue
 * belong to Core. Not saveable: file names and grants never enter saved UI state.
 */
/** A write whose reply was lost. The request ID was created before submission,
 * so the journal can be asked about it and a resubmission never repeats a job. */
class UncertainWrite internal constructor(val requestId: String, internal val submit: suspend (String) -> CoreResult<JobItem>,
    internal val attentionId: String? = null) {
    /** null = not checked yet; true = Core has no such request (safe to send again with the same ID). */
    var notInJournal by mutableStateOf<Boolean?>(null)
        internal set
}

class AppSession(val core: CoreGateway, private val scope: CoroutineScope, private val newRequestId: () -> String = ::randomRequestId,
    catalogStore: VaultCatalogStore = MemoryVaultCatalogStore(), placesStore: PlaceCatalogStore = MemoryPlaceCatalogStore()) {
    val vaultCatalog = VaultCatalog(catalogStore, scope)
    val places = PlaceCatalog(placesStore, scope)
    val archives = ArchiveSession(core, scope)
    var fileBatch by mutableStateOf<FileBatchReport?>(null)
        private set
    private var stopBatchSubmission = false
    var directWrite by mutableStateOf<DirectWriteState?>(null)
        private set
    var fileWorkspace by mutableStateOf(FileWorkspace())
        private set
    var files: FilesState
        get() = fileWorkspace.current.files
        private set(value) { fileWorkspace = fileWorkspace.files(fileWorkspace.active, value) }
    var jobs by mutableStateOf<List<JobItem>>(emptyList())
        private set
    var jobsLoaded by mutableStateOf(false)
        private set
    var jobsFault by mutableStateOf<CoreFault?>(null)
        private set
    var jobsLoading by mutableStateOf(false)
        private set
    var writing by mutableStateOf(false)
        private set
    var writeFault by mutableStateOf<CoreFault?>(null)
        private set
    var uncertain by mutableStateOf<UncertainWrite?>(null)
        private set
    var attention by mutableStateOf(AttentionState())
        private set
    var uncertainAttention by mutableStateOf<UncertainAttention?>(null)
        private set
    // Only IDs survive pause; descriptions and paths are dropped with the screen state.
    private var attentionResolutions by mutableStateOf<Map<String, String>>(emptyMap())
    private var attentionReady by mutableStateOf<Set<String>>(emptySet())
    private var inFlightAttention: UncertainAttention? = null
    private var attentionRevision = 0L
    /** Bumped on every Quick Lock: notes and encrypted-folder screens drop decrypted state. */
    var secretsEpoch by mutableStateOf(0L)
        private set
    var protectedWrite by mutableStateOf(ProtectedWriteState())
        private set
    fun beginProtectedWrite(): Long? {
        val next = protectedWrite.begin() ?: return null
        protectedWrite = next
        return next.ticket
    }
    fun finishProtectedWrite(ticket: Long, uncertain: Boolean) {
        protectedWrite = protectedWrite.finish(ticket, uncertain)
    }
    fun acknowledgeProtectedWrite() { protectedWrite = protectedWrite.acknowledge() }
    private var polling = false
    private var inFlight: UncertainWrite? = null
    private var revision = 0L
    private val tabRevisions = mutableMapOf<String, Long>()
    private var epoch = 0L
    private var spacesRevision = 0L
    private var workJob = SupervisorJob(scope.coroutineContext[Job])
    private val workScope: CoroutineScope get() = CoroutineScope(scope.coroutineContext + workJob)

    fun resume() { loadSpaces(); refreshJobs(); refreshAttention(); reconnectCloud() }
    /** Stops UI awaits, not durable Rust operations. No automatic write retry. */
    fun pause() {
        protectedWrite = protectedWrite.interrupted()
        fileBatch = fileBatch?.interrupted()?.redact()
        stopBatchSubmission = true
        core.releasePdf()
        core.lockSecrets()
        archives.close()
        if (directWrite?.phase == DirectWritePhase.Running) directWrite = directWrite?.copy(phase = DirectWritePhase.Unknown)
        ++secretsEpoch
        ++epoch
        ++revision
        ++spacesRevision
        ++attentionRevision
        workJob.cancel()
        workJob = SupervisorJob(scope.coroutineContext[Job])
        tabRevisions.clear()
        fileWorkspace = FileWorkspace()
        jobs = emptyList()
        jobsLoaded = false
        jobsFault = null
        jobsLoading = false
        attention = AttentionState()
        polling = false
        if (writing) {
            writeFault = CoreFault.OutcomeUnknown
            inFlight?.let { uncertain = it }
            inFlightAttention?.let { uncertainAttention = it }
        }
        inFlight = null
        inFlightAttention = null
        writing = false
    }

    private suspend fun <T> read(block: suspend () -> CoreResult<T>): CoreResult<T> = try {
        block()
    } catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { CoreResult.Failure(CoreFault.Unavailable) }

    fun loadSpaces() {
        val request = ++spacesRevision
        workScope.launch {
            val result = read { core.spaces() }
            if (request != spacesRevision) return@launch
            when (result) {
                is CoreResult.Value -> files = files.copy(spaces = result.value)
                is CoreResult.Failure -> files = files.copy(fault = result.fault)
            }
        }
    }
    /** A callback keeps a vault picker in its own flow instead of opening Files.
     * Lost attachment replies are reported once; never retried automatically. */
    fun attach(folder: PickedFolder, onResult: ((CoreResult<Space>) -> Unit)? = null) {
        if (writing || !core.connected) {
            onResult?.invoke(CoreResult.Failure(if (writing) CoreFault.Busy else CoreFault.Disconnected))
            return
        }
        ++spacesRevision
        val requestEpoch = epoch
        writing = true
        // Enter try/finally before suspension so a lock cannot strand the picker.
        workScope.launch(start = CoroutineStart.UNDISPATCHED) {
            var reply: CoreResult<Space> = CoreResult.Failure(CoreFault.OutcomeUnknown)
            try {
                val result = read { core.attachFolder(folder) }
                if (requestEpoch != epoch) return@launch
                reply = result
                when (result) {
                    is CoreResult.Value -> {
                        ++spacesRevision // Discard space lists read before this attachment completed.
                        files = files.copy(spaces = files.spaces.filterNot { it.id == result.value.id } + result.value)
                        if (onResult == null) openSpace(result.value)
                    }
                    is CoreResult.Failure -> writeFault = result.fault
                }
            } finally {
                if (requestEpoch == epoch) writing = false
                onResult?.invoke(reply)
            }
        }
    }
    /** Saved servers: connected off the start path (network); unreachable ones stay saved. */
    private fun reconnectCloud() {
        val requestEpoch = epoch
        workScope.launch {
            val result = read { core.reconnectCloud() }
            // Re-read the full list: a concurrent loadSpaces may have read it before the reconnect.
            if (requestEpoch == epoch && result is CoreResult.Value && result.value.isNotEmpty()) loadSpaces()
        }
    }
    /** Own server over WebDAV. Persisted only after the server answered. Not retried automatically. */
    fun attachWebDav(url: String, user: String, token: String) {
        if (writing || !core.connected) return
        ++spacesRevision
        val requestEpoch = epoch
        writing = true
        writeFault = null
        workScope.launch {
            try {
                val result = read { core.attachWebDav(url, user, token) }
                if (requestEpoch != epoch) return@launch
                when (result) {
                    is CoreResult.Value -> {
                        files = files.copy(spaces = files.spaces.filterNot { it.id == result.value.id } + result.value)
                        openSpace(result.value)
                    }
                    is CoreResult.Failure -> writeFault = result.fault
                }
            } finally { if (requestEpoch == epoch) writing = false }
        }
    }
    /** Disconnect a folder. Core refuses (Busy) while a queued or running job uses it. */
    fun detach(space: Space) {
        if (writing || !core.connected) return
        ++spacesRevision
        val requestEpoch = epoch
        writing = true
        writeFault = null
        workScope.launch {
            try {
                val result = read { core.detach(space) }
                if (requestEpoch != epoch) return@launch
                when (result) {
                    is CoreResult.Value -> {
                        fileWorkspace.tabs.filter { it.files.folder?.storageId == space.id }.forEach { tabRevisions.remove(it.id) }
                        fileWorkspace = fileWorkspace.withoutSpace(space.id)
                    }
                    is CoreResult.Failure -> writeFault = result.fault
                }
            } finally { if (requestEpoch == epoch) writing = false }
        }
    }
    fun openSpace(space: Space) = browse(listOf(FolderNode(space.root, space.name)))
    fun openFolder(item: FileItem) {
        if (item.kind == EntryKind.Directory) browse(files.trail + FolderNode(item.location, item.name))
    }
    fun breadcrumb(index: Int) {
        if (index in files.trail.indices) browse(files.trail.take(index + 1))
    }
    fun refresh() { if (files.trail.isNotEmpty()) browse(files.trail) else loadSpaces() }
    fun refreshFileTabs() {
        val tabs = fileWorkspace.tabs.filter { it.files.trail.isNotEmpty() }
        if (tabs.isEmpty()) loadSpaces() else tabs.forEach { browse(it.files.trail, it.id) }
    }
    fun activateFileTab(id: String): Boolean {
        if (fileWorkspace.tab(id) == null) return false
        fileWorkspace = fileWorkspace.focus(id)
        val state = files
        if (!state.loaded && !state.loading && state.trail.isNotEmpty()) browse(state.trail, id)
        return true
    }
    fun newFileTab(duplicate: Boolean = false) {
        if (fileWorkspace.tabs.size >= MAX_FILE_TABS) { writeFault = CoreFault.Limit; return }
        fileWorkspace = fileWorkspace.add(duplicate)
        activateFileTab(fileWorkspace.active)
    }
    fun splitFilePanels(enabled: Boolean) {
        fileWorkspace = fileWorkspace.split(enabled)
        listOfNotNull(fileWorkspace.left, fileWorkspace.right).forEach { id ->
            val state = fileWorkspace.tab(id)?.files ?: return@forEach
            if (!state.loaded && !state.loading && state.trail.isNotEmpty()) browse(state.trail, id)
        }
    }
    fun moveFileTabToOtherPane(id: String) { fileWorkspace = fileWorkspace.moveToOtherPane(id); activateFileTab(fileWorkspace.active) }
    fun closeFileTab(id: String) {
        if (fileWorkspace.tabs.size <= 1) return
        fileWorkspace = fileWorkspace.close(id); tabRevisions.remove(id)
        activateFileTab(fileWorkspace.active)
    }
    fun keepFileTabs(ids: Set<String>) {
        fileWorkspace = fileWorkspace.keep(ids)
        tabRevisions.keys.retainAll(ids)
        activateFileTab(fileWorkspace.active)
    }
    fun fileOptions(id: String, options: FileListOptions) {
        if (activateFileTab(id)) fileWorkspace = fileWorkspace.update(id) { it.copy(options = options) }
    }
    fun fileView(id: String, view: FileView) {
        if (activateFileTab(id)) fileWorkspace = fileWorkspace.update(id) { it.copy(view = view) }
    }
    fun copyBetweenFileTabs(sourceTab: String, source: FileItem, targetTab: String, parent: Location, conflict: NameConflict) {
        if (writing || !core.connected) return
        if (fileBatch != null || uncertain != null || directWrite?.phase == DirectWritePhase.Unknown) { writeFault = CoreFault.Busy; return }
        val from = fileWorkspace.tab(sourceTab)?.files
        val to = fileWorkspace.tab(targetTab)?.files
        if (from == null || to == null || !from.loaded || from.loading || source !in from.entries ||
            !to.loaded || to.loading || to.folder != parent) { writeFault = CoreFault.Changed; return }
        if (conflict == NameConflict.Replace || !source.canTransferTo(parent, TransferMode.Copy, to.spaces) ||
            (source.isTarget(parent, source.name) && conflict != NameConflict.KeepBoth)) { writeFault = CoreFault.Unsupported; return }
        fileWorkspace = fileWorkspace.focus(targetTab)
        submit { id -> core.transfer(source.location, parent, source.name, TransferMode.Copy, conflict, id) }
    }
    private fun browse(trail: List<FolderNode>, tabId: String = fileWorkspace.active) {
        val state = fileWorkspace.tab(tabId)?.files ?: return
        val request = ++revision
        tabRevisions[tabId] = request
        fileWorkspace = fileWorkspace.files(tabId, state.copy(trail = trail, entries = emptyList(), hidden = 0, selected = emptySet(), loading = true, loaded = false, fault = null))
        workScope.launch {
            val result = read { core.list(trail.last().location) }
            if (request != tabRevisions[tabId]) return@launch
            val current = fileWorkspace.tab(tabId)?.files ?: return@launch
            val next = when (result) {
                is CoreResult.Value -> current.copy(entries = result.value.items, hidden = result.value.hidden, loading = false, loaded = true)
                is CoreResult.Failure -> current.copy(loading = false, fault = result.fault)
            }
            fileWorkspace = fileWorkspace.files(tabId, next)
        }
    }
    fun select(location: Location) {
        if (files.entries.none { it.location == location }) return
        if (location !in files.selected && files.selected.size >= MAX_FILE_SELECTION) {
            writeFault = CoreFault.Limit; return
        }
        files = files.copy(selected = if (location in files.selected) files.selected - location else files.selected + location)
    }
    fun clearSelection() { files = files.copy(selected = emptySet()) }
    /** Filters affect display only. Existing hidden selections are kept and counted. */
    fun selectVisible(locations: List<Location>) {
        if (!files.loaded || files.loading || locations.any { location -> files.entries.none { it.location == location } }) return
        val selection = files.selected + locations
        if (selection.size > MAX_FILE_SELECTION) { writeFault = CoreFault.Limit; return }
        files = files.copy(selected = selection)
    }
    fun submitFileBatch(plan: FileBatchPlan) {
        if (writing || !core.connected) return
        if (fileBatch != null || uncertain != null || uncertainAttention != null || directWrite?.phase == DirectWritePhase.Unknown) {
            writeFault = CoreFault.Busy; return
        }
        val frozen = plan.copy(sources = plan.sources.toList())
        if (!files.loaded || files.loading || frozen.sources.any { it !in files.entries } ||
            frozen.sources.map { it.location }.toSet() != files.selected) {
            writeFault = CoreFault.Changed; return
        }
        if (!frozen.allowed(files.spaces)) { writeFault = CoreFault.Unsupported; return }
        val ids = List(frozen.sources.size) { newRequestId() }
        val requestEpoch = epoch
        stopBatchSubmission = false
        writing = true; writeFault = null
        workScope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                FileBatchDispatcher(core).send(frozen, ids, shouldStop = { stopBatchSubmission }, update = { report ->
                    if (requestEpoch == epoch) fileBatch = report.copy(stopRequested = report.stopRequested || stopBatchSubmission)
                }, accepted = { job ->
                    if (requestEpoch == epoch) jobs = listOf(job) + jobs.filterNot { it.id == job.id }
                })
            } finally {
                if (requestEpoch == epoch) {
                    writing = false
                    refreshFileTabs(); refreshJobs(); refreshAttention(); finishAttentionResolutions()
                }
            }
        }
    }
    fun stopFileBatch() {
        if (fileBatch?.sending != true) return
        stopBatchSubmission = true
        fileBatch = fileBatch?.copy(stopRequested = true)
    }
    /** Closing this transient report never cancels or resubmits journal jobs. */
    fun dismissFileBatch() { if (fileBatch?.sending == false) fileBatch = null }
    fun dismissWriteFault() { writeFault = null; uncertain = null }
    fun createDirectory(parent: Location, name: String) = submit { id -> core.createDirectory(parent, name, id) }
    fun createText(parent: Location, name: String) = writeNewText(parent, name, "")
    fun saveDiagnosticReport(parent: Location, report: DiagnosticBundle) = writeNewText(parent, report.filename, report.text)
    private fun writeNewText(parent: Location, name: String, text: String) {
        if (writing || !core.connected) return
        if (!files.loaded || files.loading || files.folder != parent) { writeFault = CoreFault.Changed; return }
        if (!files.writable) { writeFault = CoreFault.Permission; return }
        if (!validNewTextName(name)) { writeFault = CoreFault.InvalidName; return }
        directFileWrite(DirectWriteKind.CreateText) {
            when (val result = core.textSaveCopy(parent, name, text)) {
                is CoreResult.Value -> CoreResult.Value(Unit)
                is CoreResult.Failure -> result
            }
        }
    }
    fun copy(source: Location, parent: Location, name: String) = submit { id -> core.copy(source, parent, name, id) }
    fun dismissDirectWrite() {
        if (directWrite?.phase != DirectWritePhase.Running) {
            directWrite = null
            if (uncertain == null && uncertainAttention == null) writeFault = null
        }
    }
    fun transformFile(source: FileItem, parent: Location, name: String, format: FileCryptoFormat,
        decrypt: Boolean, password: String) {
        if (writing || !core.connected || !currentSource(source)) return
        if (source.kind != EntryKind.File || files.spaces.spaceOf(parent)?.writable != true) {
            writeFault = CoreFault.Unsupported; return
        }
        if (!plausibleOperationName(name) || source.isTarget(parent, name) || password.isEmpty() || (!decrypt && password.length < 12)) {
            writeFault = CoreFault.InvalidName; return
        }
        directFileWrite(if (decrypt) DirectWriteKind.DecryptFile else DirectWriteKind.EncryptFile) {
            if (decrypt) core.decryptFile(source.location, parent, name, format, password)
            else core.encryptFile(source.location, parent, name, format, password)
        }
    }
    fun openArchive(source: FileItem, format: ArchiveFormat, password: String?): Boolean {
        if (writing || !core.connected || !currentSource(source)) return false
        if (source.kind != EntryKind.File) { writeFault = CoreFault.Unsupported; return false }
        archives.open(source, format, password)
        return true
    }
    fun createArchive(sources: List<FileItem>, format: ArchiveFormat, password: String?, parent: Location, name: String) {
        if (writing || !core.connected) return
        if (!files.loaded || files.loading || sources.isEmpty() || sources.any { it !in files.entries || !it.canCopy(files.spaces) }) {
            writeFault = CoreFault.Changed; return
        }
        if (!plausibleOperationName(name)) { writeFault = CoreFault.InvalidName; return }
        if (files.spaces.spaceOf(parent)?.writable != true || (format == ArchiveFormat.SevenZip && !password.isNullOrEmpty())) {
            writeFault = CoreFault.Unsupported; return
        }
        val locations = sources.map { it.location }
        val id = newRequestId()
        directFileWrite(DirectWriteKind.CreateArchive, id) { core.archiveCreateRequested(locations, format, password, parent, name, id) }
    }
    fun extractArchive(source: FileItem, format: ArchiveFormat, password: String?, parent: Location, name: String) {
        if (writing || !core.connected) return
        if (archives.state.source != source || archives.state.format != format) { writeFault = CoreFault.Changed; return }
        if (!plausibleOperationName(name)) { writeFault = CoreFault.InvalidName; return }
        if (files.spaces.spaceOf(parent)?.writable != true) { writeFault = CoreFault.Permission; return }
        val id = newRequestId()
        directFileWrite(DirectWriteKind.ExtractArchive, id) { core.archiveExtractRequested(source.location, format, password, parent, name, id) }
    }
    fun cancelDirectArchive() {
        val operation = directWrite ?: return
        val id = operation.requestId ?: return
        if (operation.phase != DirectWritePhase.Running || operation.cancelling || !core.connected) return
        val requestEpoch = epoch
        directWrite = operation.copy(cancelling = true)
        workScope.launch {
            val result = read { core.cancelArchiveRequested(id) }
            if (requestEpoch == epoch && directWrite?.requestId == id && directWrite?.phase == DirectWritePhase.Running && result is CoreResult.Failure) {
                writeFault = result.fault
                directWrite = directWrite?.copy(cancelling = false)
            }
        }
    }
    private fun directFileWrite(kind: DirectWriteKind, requestId: String? = null, block: suspend () -> CoreResult<Unit>) {
        if (writing || !core.connected || directWrite?.phase == DirectWritePhase.Unknown || uncertain != null || uncertainAttention != null) return
        val requestEpoch = epoch
        writing = true; writeFault = null
        directWrite = DirectWriteState(kind, DirectWritePhase.Running, requestId)
        workScope.launch {
            try {
                val result = try {
                    if (requestId != null && directWrite?.cancelling == true) CoreResult.Failure(CoreFault.Cancelled)
                    else block()
                }
                    catch (cancelled: CancellationException) { throw cancelled }
                    catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }
                if (requestEpoch != epoch) return@launch
                directWrite = DirectWriteState(kind, when (result) {
                    is CoreResult.Value -> DirectWritePhase.Done
                    is CoreResult.Failure -> {
                        writeFault = result.fault
                        if (result.fault == CoreFault.OutcomeUnknown) DirectWritePhase.Unknown else DirectWritePhase.Failed
                    }
                }, requestId)
                refreshFileTabs(); refreshJobs(); refreshAttention()
            } finally { if (requestEpoch == epoch) { writing = false; finishAttentionResolutions() } }
        }
    }
    /** Source is the displayed snapshot; destructive actions do not silently use a new selection. */
    private fun currentSource(source: FileItem): Boolean {
        if (!files.loaded || files.loading || source !in files.entries) {
            writeFault = CoreFault.Changed
            return false
        }
        return true
    }
    fun transfer(source: FileItem, parent: Location, name: String, mode: TransferMode,
        conflict: NameConflict, confirmedReplacement: FileItem? = null) {
        if (writing || !core.connected || !currentSource(source)) return
        if (!plausibleOperationName(name)) {
            writeFault = CoreFault.InvalidName; return
        }
        if (!source.canTransferTo(parent, mode, files.spaces) ||
            (mode == TransferMode.Move && (conflict != NameConflict.Fail || source.isTarget(parent, name))) ||
            (conflict == NameConflict.Replace && (mode != TransferMode.Copy ||
                !source.canReplace(confirmedReplacement, parent, name, files.spaces)))) {
            writeFault = CoreFault.Unsupported; return
        }
        submit { id -> core.transfer(source.location, parent, name, mode, conflict, id) }
    }
    fun rename(source: FileItem, name: String) {
        if (writing || !core.connected || !currentSource(source)) return
        if (!plausibleOperationName(name) || name == source.name) {
            writeFault = CoreFault.InvalidName; return
        }
        if (!source.canRename(files.spaces)) { writeFault = CoreFault.Unsupported; return }
        submit { id -> core.rename(source.location, name, id) }
    }
    fun delete(source: FileItem, recursive: Boolean = false) {
        if (writing || !core.connected || !currentSource(source)) return
        if (!source.canDelete(files.spaces) || (recursive && source.kind != EntryKind.Directory)) {
            writeFault = CoreFault.Unsupported; return
        }
        submit { id -> core.delete(source.location, recursive, id) }
    }
    private fun submit(attentionId: String? = null, request: suspend (String) -> CoreResult<JobItem>) {
        val pending = UncertainWrite(newRequestId(), request, attentionId)
        // Kept until the reply arrives; a lost reply (or pause) leaves it as `uncertain`.
        write(pending) { request(pending.requestId) }
    }
    /** Ask the journal about the uncertain request. Never resubmits by itself. */
    fun checkUncertain() {
        val pending = uncertain ?: return
        if (writing || !core.connected) return
        val requestEpoch = epoch
        writing = true
        workScope.launch {
            try {
                val result = read { core.job(pending.requestId) }
                if (requestEpoch != epoch) return@launch
                when (result) {
                    is CoreResult.Value -> {
                        val job = result.value
                        if (job == null) pending.notInJournal = true
                        else { uncertain = null; writeFault = null; accepted(job, pending.attentionId) }
                    }
                    is CoreResult.Failure -> writeFault = result.fault
                }
            } finally { if (requestEpoch == epoch) { writing = false; finishAttentionResolutions() } }
        }
    }
    /** Explicit user action, only after the journal reported the request absent.
     * Same request ID: if the first submission did land meanwhile, Core returns it. */
    fun resendUncertain() {
        val pending = uncertain ?: return
        if (pending.notInJournal != true || writing) return
        uncertain = null
        pending.notInJournal = null
        write(pending) { pending.submit(pending.requestId) }
    }
    private fun accepted(job: JobItem, attentionId: String? = null) {
        if (attentionId != null) attentionResolutions = attentionResolutions + (job.id to attentionId)
        jobs = listOf(job) + jobs.filterNot { it.id == job.id }
        observeAttentionResolutions(listOf(job))
        if (job.phase == JobPhase.NeedsAttention) refreshAttention()
        // Submission is not completion: follow the job until it finishes.
        refresh()
        poll(epoch)
    }
    private fun write(request: UncertainWrite? = null, block: suspend () -> CoreResult<JobItem>) {
        if (writing || !core.connected) return
        writing = true
        writeFault = null
        inFlight = request
        val requestEpoch = epoch
        workScope.launch {
            try {
                // An exception crossing a write boundary is NOT proof of no side effect.
                val result = try { block() }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }
                // A late reply after pause: pause already recorded the request as uncertain.
                if (requestEpoch != epoch) return@launch
                inFlight = null
                when (result) {
                    is CoreResult.Value -> accepted(result.value, request?.attentionId)
                    is CoreResult.Failure -> {
                        writeFault = result.fault
                        if (result.fault == CoreFault.OutcomeUnknown && request != null) uncertain = request
                    }
                }
            } finally { if (requestEpoch == epoch) { writing = false; inFlight = null; finishAttentionResolutions() } }
        }
    }
    fun refreshAttention(reconcileDecision: Boolean = false) {
        val request = ++attentionRevision
        val requestEpoch = epoch
        val unknown = if (reconcileDecision) uncertainAttention else null
        attention = attention.copy(loading = true, fault = null)
        workScope.launch {
            val result = read { core.attentionList() }
            if (requestEpoch != epoch || request != attentionRevision) return@launch
            attention = when (result) {
                is CoreResult.Value -> {
                    // A read only: if still open the user can choose again; no decision is repeated here.
                    if (unknown != null && uncertainAttention == unknown) {
                        uncertainAttention = null
                        if (uncertain == null && writeFault == CoreFault.OutcomeUnknown) writeFault = null
                    }
                    AttentionState(result.value, loaded = true)
                }
                is CoreResult.Failure -> attention.copy(loading = false, fault = result.fault)
            }
        }
    }
    fun checkAttentionDecision() {
        if (!writing && core.connected && uncertainAttention != null) refreshAttention(reconcileDecision = true)
    }
    fun attentionPending(jobId: String): Boolean = jobId in attentionResolutions.values || jobId in attentionReady ||
        uncertain?.attentionId == jobId || inFlight?.attentionId == jobId ||
        uncertainAttention?.jobId == jobId || inFlightAttention?.jobId == jobId

    /** Opens a logical resource parent. No native path or OS file access is reconstructed. */
    fun openAttentionFolder(location: Location): Boolean {
        val space = files.spaces.spaceOf(location) ?: run { writeFault = CoreFault.Unavailable; return false }
        val parts = if (location.path.isEmpty()) emptyList() else location.path.split('/').dropLast(1)
        browse(listOf(FolderNode(space.root, space.name)) + parts.mapIndexed { index, name ->
            FolderNode(Location(location.storageId, parts.take(index + 1).joinToString("/")), name)
        })
        return true
    }
    fun resolveAttention(item: AttentionItem, action: AttentionAction) {
        if (writing || !core.connected || uncertain != null || uncertainAttention != null || attentionPending(item.job.id)) return
        if (attention.loading || !attention.loaded || attention.fault != null || item !in attention.items) {
            writeFault = CoreFault.Changed; return
        }
        val allowed = when (action) {
            AttentionAction.Restore -> item.canRestore
            AttentionAction.Discard -> item.canDiscard
            AttentionAction.Accept -> item.canAccept
            AttentionAction.Reviewed -> true
        }
        if (!allowed) { writeFault = CoreFault.Unsupported; return }
        when (action) {
            AttentionAction.Restore -> submit(item.job.id) { core.attentionRestore(item.job.id, it) }
            AttentionAction.Discard -> submit(item.job.id) { core.attentionDiscard(item.job.id, it) }
            AttentionAction.Accept, AttentionAction.Reviewed -> decideAttention(item.job.id, action)
        }
    }
    /** The two Unit-returning decisions have no request ID. A lost reply is checked by reading,
     * never retried automatically, including after pause or a failed automatic Reviewed. */
    private fun decideAttention(jobId: String, action: AttentionAction) {
        if (writing || !core.connected || uncertainAttention != null || uncertain != null) return
        val decision = UncertainAttention(jobId, action)
        val requestEpoch = epoch
        writing = true
        writeFault = null
        inFlightAttention = decision
        workScope.launch {
            try {
                val result = try {
                    when (action) {
                        AttentionAction.Reviewed -> core.attentionReviewed(jobId)
                        AttentionAction.Accept -> core.attentionAccept(jobId)
                        else -> CoreResult.Failure(CoreFault.Unsupported)
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }
                if (requestEpoch != epoch) return@launch
                if (result is CoreResult.Failure) {
                    writeFault = result.fault
                    if (result.fault == CoreFault.OutcomeUnknown) uncertainAttention = decision
                }
                refreshAttention(); refreshJobs(); refresh()
            } finally {
                if (requestEpoch == epoch) {
                    writing = false; inFlightAttention = null
                    finishAttentionResolutions()
                }
            }
        }
    }
    private fun observeAttentionResolutions(snapshots: List<JobItem>) {
        var finished = false
        snapshots.forEach { job ->
            val origin = attentionResolutions[job.id] ?: return@forEach
            if (!job.active) {
                attentionResolutions = attentionResolutions - job.id
                // An accepted submission, a failed/cancelled job, or NeedsAttention never closes the origin.
                if (job.phase == JobPhase.Succeeded) attentionReady = attentionReady + origin
                finished = true
            }
        }
        if (finished) refreshAttention()
    }
    private fun finishAttentionResolutions() {
        if (writing || !core.connected || uncertain != null || uncertainAttention != null) return
        val origin = attentionReady.firstOrNull() ?: return
        attentionReady = attentionReady - origin
        // Authorized by the explicit Restore/Discard action; only after its durable job succeeded.
        decideAttention(origin, AttentionAction.Reviewed)
    }
    fun refreshJobs() {
        if (jobsLoading) return
        jobsLoading = true
        val requestEpoch = epoch
        workScope.launch {
            try {
                val result = read { core.jobs() }
                if (requestEpoch != epoch) return@launch
                when (result) {
                    is CoreResult.Value -> {
                        val previousJobs = jobs
                        jobs = result.value; jobsLoaded = true; jobsFault = null
                        // A recovery job can fall outside the recent-jobs window. Query its durable ID too.
                        val tracked = mutableListOf<JobItem>()
                        val batchIds = fileBatch?.entries?.filter {
                            it.delivery == BatchDelivery.Accepted || it.delivery == BatchDelivery.Unknown
                        }?.map { it.jobId ?: it.requestId }.orEmpty()
                        for (id in (attentionResolutions.keys + batchIds).distinct().filter { id -> jobs.none { it.id == id } }) {
                            val found = read { core.job(id) }
                            if (requestEpoch != epoch) return@launch
                            if (found is CoreResult.Value) found.value?.let { tracked += it }
                            else if (found is CoreResult.Failure) jobsFault = found.fault
                        }
                        jobs = jobs + tracked
                        val finished = previousJobs.any { old -> old.active && jobs.any { it.id == old.id && !it.active } }
                        fileBatch = fileBatch?.reconcile(jobs)
                        observeAttentionResolutions(jobs)
                        if (finished) { refreshFileTabs(); refreshAttention() }
                    }
                    is CoreResult.Failure -> jobsFault = result.fault
                }
            } finally { if (requestEpoch == epoch) { jobsLoading = false; finishAttentionResolutions() } }
            if (requestEpoch == epoch) poll(requestEpoch)
        }
    }
    /** Bounded follow-up while jobs are active: one poller, stops when nothing
     * is queued/running, on failure, or on pause. Not a write retry. */
    private fun poll(requestEpoch: Long) {
        if (polling || jobsFault != null || jobs.none { it.active }) return
        polling = true
        workScope.launch {
            try { delay(POLL_MS) } finally { if (requestEpoch == epoch) polling = false }
            if (requestEpoch == epoch) refreshJobs()
        }
    }
    fun cancel(job: JobItem) {
        if (job.canCancel) write { core.cancel(job.id) }
    }
    private companion object { const val POLL_MS = 2000L }
}


private val JobItem.active get() = phase == JobPhase.Queued || phase == JobPhase.Running
@OptIn(kotlin.uuid.ExperimentalUuidApi::class)
internal fun randomRequestId(): String = kotlin.uuid.Uuid.random().toString()

/** Caller must save drafts before entering. Reconciliation is an explicit UI action. */
suspend fun <T> protectedWrite(session: AppSession, action: suspend () -> CoreResult<T>): CoreResult<T> {
    val ticket = session.beginProtectedWrite() ?: return CoreResult.Failure(
        if (session.protectedWrite.phase == ProtectedWritePhase.Unknown) CoreFault.OutcomeUnknown else CoreFault.Busy)
    var uncertain = true
    try {
        val result = try { action() }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { CoreResult.Failure(CoreFault.OutcomeUnknown) }
        uncertain = result is CoreResult.Failure && result.fault.hasUnknownWriteOutcome()
        return result
    } finally { session.finishProtectedWrite(ticket, uncertain) }
}
