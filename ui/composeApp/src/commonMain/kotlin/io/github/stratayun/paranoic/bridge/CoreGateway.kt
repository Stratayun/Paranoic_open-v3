package io.github.stratayun.paranoic.bridge

/** Presentation contract for the NEW Core; deliberately not a native ABI.
 * Only the future platform binding may import generated UniFFI/Gobley symbols.
 * Calls must be main-safe. Native errors must be redacted into CoreFault.
 */
interface CoreGateway {
    val connected: Boolean
    /** Presentation barrier: wait for earlier protected native IO to actually finish. */
    suspend fun awaitProtectedWrites(): CoreResult<Unit> = unsupported()
    suspend fun spaces(): CoreResult<List<Space>>
    suspend fun attachFolder(folder: PickedFolder): CoreResult<Space>
    /** Display listing; entries Core cannot represent safely are counted in `hidden`. */
    suspend fun list(folder: Location): CoreResult<FolderListing>
    /** Disconnect a folder: Core refuses (Busy) while a queued/running job uses it.
     * Releases the OS permission and removes the grant from the protected store. */
    suspend fun detach(space: Space): CoreResult<Unit>
    /** requestId: created BEFORE submission and kept by the caller. Submitting
     * again with the same ID never repeats a job (Core returns the existing one). */
    suspend fun createDirectory(parent: Location, name: String, requestId: String): CoreResult<JobItem>
    /** Must submit create-new/fail-on-conflict CopyFile/CopyDirectory in Rust. */
    suspend fun copy(source: Location, parent: Location, name: String, requestId: String): CoreResult<JobItem>
    /** Copy supports conflict policies; Move always uses Fail. All naming/IO stays in Rust. */
    suspend fun transfer(source: Location, parent: Location, name: String, mode: TransferMode,
        conflict: NameConflict, requestId: String): CoreResult<JobItem> = unsupported()
    suspend fun rename(source: Location, name: String, requestId: String): CoreResult<JobItem> = unsupported()
    suspend fun delete(source: Location, recursive: Boolean, requestId: String): CoreResult<JobItem> = unsupported()
    suspend fun jobs(): CoreResult<List<JobItem>>
    /** Journal lookup by request ID; Value(null) = the request never reached the journal. */
    suspend fun job(id: String): CoreResult<JobItem?>
    suspend fun cancel(jobId: String): CoreResult<JobItem>
    suspend fun attentionList(): CoreResult<List<AttentionItem>> = unsupported()
    suspend fun attentionRestore(jobId: String, requestId: String): CoreResult<JobItem> = unsupported()
    suspend fun attentionDiscard(jobId: String, requestId: String): CoreResult<JobItem> = unsupported()
    /** Journal decisions only. Never call automatically except Reviewed after a successful explicit recovery job. */
    suspend fun attentionReviewed(jobId: String): CoreResult<Unit> = unsupported()
    suspend fun attentionAccept(jobId: String): CoreResult<Unit> = unsupported()
    suspend fun inspect(location: Location): CoreResult<PreviewInfo>
    suspend fun textOpen(location: Location): CoreResult<TextDocument> = unsupported()
    /** Read-only indexed text, up to the Core limit. Close every returned handle,
     * including an open that completes after the caller has been cancelled. */
    suspend fun textPagerOpen(location: Location): CoreResult<TextPager> = unsupported()
    suspend fun textPagerRead(handle: String, first: Long, count: Int): CoreResult<List<String>> = unsupported()
    suspend fun textPagerClose(handle: String): CoreResult<Unit> = unsupported()
    /** Password analysis and entropy come from Rust; never log either argument or result. */
    suspend fun passwordFeedback(password: String): CoreResult<PasswordFeedback> = unsupported()
    suspend fun generatePassphrase(words: Int = 6): CoreResult<String> = unsupported()
    suspend fun textSave(location: Location, version: TextFileVersion, text: String): CoreResult<TextFileVersion> = unsupported()
    /** Creates a new UTF-8 copy; never replaces a target or changes the open document. */
    suspend fun textSaveCopy(parent: Location, name: String, text: String): CoreResult<TextFileVersion> = unsupported()
    /** Bounded, structured read-only content. Format validation and all IO stay in Core. */
    suspend fun officeOpen(location: Location): CoreResult<OfficeDocument> = unsupported()
    suspend fun imagePng(location: Location, side: Int): CoreResult<ByteArray>
    suspend fun pdfPage(location: Location, page: Int, width: Int): CoreResult<PdfPage>
    suspend fun pdfSearch(location: Location, query: String, start: Int): CoreResult<PdfMatches>
    /** Stop the PDF worker and drop the loaded document (viewer closed, pause/lock). Never blocks the caller. */
    fun releasePdf()
    suspend fun openMedia(location: Location): CoreResult<MediaSource>
    suspend fun encryptFile(source: Location, parent: Location, name: String, format: FileCryptoFormat,
        password: String): CoreResult<Unit> = unsupported()
    suspend fun decryptFile(source: Location, parent: Location, name: String, format: FileCryptoFormat,
        password: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorCreate(parent: Location, name: String, password: String): CoreResult<String> = unsupported()
    suspend fun cryptomatorOpen(root: Location, password: String): CoreResult<String> = unsupported()
    suspend fun cryptomatorList(handle: String, path: List<String>): CoreResult<CryptomatorListing> = unsupported()
    suspend fun cryptomatorAdd(handle: String, path: List<String>, source: Location): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorOpenStream(handle: String, path: List<String>, name: String): CoreResult<MediaSource> = unsupported()
    suspend fun cryptomatorPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int): CoreResult<PdfPage> = unsupported()
    suspend fun cryptomatorPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int): CoreResult<PdfMatches> = unsupported()
    suspend fun cryptomatorOfficeOpen(handle: String, path: List<String>, name: String): CoreResult<OfficeDocument> = unsupported()
    suspend fun cryptomatorTextOpen(handle: String, path: List<String>, name: String): CoreResult<VaultTextDocument> = unsupported()
    suspend fun cryptomatorTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorTextCreate(handle: String, path: List<String>, name: String, text: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?): CoreResult<List<ArchiveEntry>> = unsupported()
    suspend fun cryptomatorArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?,
        targetPath: List<String>, targetName: String): CoreResult<ULong> = unsupported()
    suspend fun cryptomatorExport(handle: String, path: List<String>, name: String, parent: Location): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorMkdir(handle: String, path: List<String>, name: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorRename(handle: String, path: List<String>, name: String, newName: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorDelete(handle: String, path: List<String>, name: String): CoreResult<Unit> = unsupported()
    suspend fun cryptomatorLock(handle: String): CoreResult<Unit> = unsupported()
    suspend fun archiveList(source: Location, format: ArchiveFormat, password: String?): CoreResult<List<ArchiveEntry>> = unsupported()
    suspend fun archiveExtract(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String): CoreResult<Unit> = unsupported()
    suspend fun archiveCreate(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String): CoreResult<Unit> = unsupported()

    // ---- Plan 3.2: recent Core errors (code + Core's fixed message; no paths,
    // names or content), oldest first, memory only. For "Подробнее" and the report.
    suspend fun diagnostics(): CoreResult<List<Diagnostic>> = unsupported()
    /** Uses the already exported, redacted Rust report. No account names or paths. */
    suspend fun diagnosticsReportText(): CoreResult<String> = unsupported()
    /** Plan 2.3: vaults in [folder] itself or in its direct subfolders, before
     * any password (marker files only). Empty: "no vault here". */
    suspend fun vaultProbe(folder: Location): CoreResult<VaultProbe> = unsupported()
    suspend fun clearDiagnostics(): CoreResult<Unit> = unsupported()

    // ---- Own server (WebDAV). The token is sealed by the device key in Rust;
    // the connection is persisted only after it worked.
    suspend fun attachWebDav(url: String, user: String, token: String): CoreResult<Space> = unsupported()
    /** Connect saved servers after start (network). Unreachable ones stay saved. */
    suspend fun reconnectCloud(): CoreResult<List<Space>> = unsupported()

    /** Quick Lock: forget the notes session and every open encrypted folder. Never blocks. */
    fun lockSecrets() = Unit

    // ---- Protected notes (1.9): isolated from files and cloud, never synced.
    suspend fun notesStatus(): CoreResult<NotesStatus> = unsupported()
    suspend fun notesPasswordStrength(password: String): CoreResult<PasswordStrength> = unsupported()
    suspend fun notesCreate(password: String): CoreResult<Unit> = unsupported()
    suspend fun notesUnlock(password: String): CoreResult<Unit> = unsupported()
    suspend fun notesQuickEnable(): CoreResult<Unit> = unsupported()
    suspend fun notesQuickDisable(): CoreResult<Unit> = unsupported()
    suspend fun notesQuickUnlock(): CoreResult<Unit> = unsupported()
    suspend fun notesLock(): CoreResult<Unit> = unsupported()
    suspend fun notesRead(): CoreResult<List<NoteTab>> = unsupported()
    suspend fun notesSave(tabs: List<NoteTab>): CoreResult<Unit> = unsupported()
    suspend fun notesChangePassword(old: String, new: String): CoreResult<Unit> = unsupported()
    /** Explicit plaintext publication into a NEW folder; attachments stay protected. */
    suspend fun notesExport(parent: Location, name: String): CoreResult<NotesExportReport> = unsupported()
    /** Adds notes, never replaces the notebook. Save UI drafts first, then reload after import. */
    suspend fun notesImport(sources: List<Location>, rootTab: String, intoTab: String?): CoreResult<NotesImportReport> = unsupported()
    suspend fun notesSetDuress(password: String?): CoreResult<Unit> = unsupported()
    suspend fun notesDuressEnabled(): CoreResult<Boolean> = unsupported()
    /** Open/create the personal 1.8 folder using the unlocked notes session. */
    suspend fun safeOpen(): CoreResult<String> = unsupported()
    /** Irreversible Clear of notes AND the personal folder, never automatically repeated. */
    suspend fun safeClear(): CoreResult<KeyGuarantee> = unsupported()
    // Sync (B4): blocking calls run off the UI thread; sync jobs appear in the jobs list.
    suspend fun syncPairs(): CoreResult<List<SyncPair>> = unsupported()
    suspend fun syncPairAdd(local: Location, remote: Location, twoWay: Boolean, trashRemoteDeletions: Boolean): CoreResult<SyncPair> = unsupported()
    suspend fun syncPairRemove(id: String): CoreResult<Unit> = unsupported()
    /** Compare both sides and save the plan; nothing is changed yet. */
    suspend fun syncPlan(id: String): CoreResult<SyncPlanSummary> = unsupported()
    /** Carry out the saved plan; repeat after an interruption (no job runs twice). */
    suspend fun syncRun(id: String): CoreResult<SyncRunResult> = unsupported()
    suspend fun syncResolve(id: String, path: String, keepLocal: Boolean): CoreResult<Unit> = unsupported()
    suspend fun syncForget(id: String, path: String): CoreResult<Unit> = unsupported()
    /** Store a file inside the notes; put the result into a note and save it. */
    suspend fun notesAttachmentAdd(name: String, data: ByteArray): CoreResult<NoteAttachment> = unsupported()
    /** Plaintext of an attachment for viewing inside the notes; wipe the array after use, never export it. */
    suspend fun notesAttachmentRead(id: String): CoreResult<ByteArray> = unsupported()
    suspend fun notesSetAutoWipe(enabled: Boolean): CoreResult<Unit> = unsupported()
    /** Destroys the notes keys; the section stays usable. Returns what was guaranteed. */
    suspend fun notesClear(): CoreResult<KeyGuarantee> = unsupported()

    // ---- Encrypted folders (format 1.8). `handle` names an unlocked folder in Rust;
    // `path` is the chain of folder names inside it. Decrypted data never passes the UI.
    suspend fun vaultCreate(parent: Location, name: String, password: String): CoreResult<String> = unsupported()
    suspend fun vaultOpen(root: Location, password: String): CoreResult<String> = unsupported()
    suspend fun vaultLock(handle: String): CoreResult<Unit> = unsupported()
    suspend fun vaultList(handle: String, path: List<String>): CoreResult<VaultListing> = unsupported()
    suspend fun vaultAdd(handle: String, path: List<String>, source: Location): CoreResult<Unit> = unsupported()
    /** requestId identifies the conditional source-removal job. Never replay an unknown write automatically. */
    suspend fun vaultMoveIn(handle: String, path: List<String>, source: Location, requestId: String): CoreResult<VaultMoveInReport> = unsupported()
    /** The implementation must own/close the one-shot input, including late/cancelled replies. */
    suspend fun vaultAddIncoming(handle: String, path: List<String>, name: String, document: IncomingDocument): CoreResult<Unit> = unsupported()
    suspend fun vaultOpenStream(handle: String, path: List<String>, name: String): CoreResult<MediaSource> = unsupported()
    suspend fun vaultPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int): CoreResult<PdfPage> = unsupported()
    suspend fun vaultPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int): CoreResult<PdfMatches> = unsupported()
    suspend fun vaultOfficeOpen(handle: String, path: List<String>, name: String): CoreResult<OfficeDocument> = unsupported()
    suspend fun vaultTextOpen(handle: String, path: List<String>, name: String): CoreResult<VaultTextDocument> = unsupported()
    suspend fun vaultTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String): CoreResult<Unit> = unsupported()
    suspend fun vaultTextCreate(handle: String, path: List<String>, name: String, text: String): CoreResult<Unit> = unsupported()
    suspend fun vaultArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?): CoreResult<List<ArchiveEntry>> = unsupported()
    /** Creates a NEW encrypted folder. A failure can leave a partly filled folder; never auto-retry. */
    suspend fun vaultArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?,
        targetPath: List<String>, targetName: String): CoreResult<ULong> = unsupported()
    /** Decrypts into `targetParent` as a new file (never overwrites), after full verification. */
    suspend fun vaultExport(handle: String, path: List<String>, name: String, targetParent: Location): CoreResult<Unit> = unsupported()
    suspend fun vaultMkdir(handle: String, path: List<String>, name: String): CoreResult<Unit> = unsupported()
    suspend fun vaultDelete(handle: String, path: List<String>, name: String, folder: Boolean): CoreResult<Unit> = unsupported()
    suspend fun vaultRename(handle: String, path: List<String>, name: String, newName: String, folder: Boolean): CoreResult<Unit> = unsupported()
    /** Moves into the folder at `targetPath`, keeping the name; never replaces (B12). */
    suspend fun vaultMove(handle: String, path: List<String>, name: String, folder: Boolean, targetPath: List<String>): CoreResult<Unit> = unsupported()
    /** Replaces the content of file `name` with `source`, only if the stored file did not change meanwhile (B12). */
    suspend fun vaultReplace(handle: String, path: List<String>, name: String, source: Location): CoreResult<Unit> = unsupported()
    /** Checks `oldPassword`, then sets the new one. Copies of the old header elsewhere still open with the old password (B11). */
    suspend fun vaultChangePassword(handle: String, oldPassword: String, newPassword: String): CoreResult<Unit> = unsupported()
    /** Creates or replaces the recovery key after checking `password`; show the returned text ONCE, never store it (B11). */
    suspend fun vaultSetRecovery(handle: String, password: String): CoreResult<String> = unsupported()
    /** Opens with the recovery key text (case, dashes and spaces ignored) (B11). */
    suspend fun vaultOpenRecovery(root: Location, recovery: String): CoreResult<String> = unsupported()
    /** Core chooses zero-based group indices; no UI random generator or key comparison. */
    suspend fun vaultRecoveryChallenge(count: Int = 3): CoreResult<List<Int>> = unsupported()
    suspend fun vaultRecoveryCheck(recovery: String, indices: List<Int>, answers: List<String>): CoreResult<Boolean> = unsupported()
    /** Requires a vault opened by the key. No automatic retry on uncertain publication. */
    suspend fun vaultResetPassword(handle: String, recovery: String, newPassword: String): CoreResult<Unit> = unsupported()
    /** Cryptomator: checks `oldPassword`, publishes the new masterkey file, then deletes the old-password backup (B11). */
    suspend fun cryptomatorChangePassword(handle: String, oldPassword: String, newPassword: String): CoreResult<Unit> = unsupported()
}
private fun unsupported() = CoreResult.Failure(CoreFault.Unsupported)

/** What protects the notes besides the password. */
enum class KeyGuarantee { PasswordOnly, DeviceKey, HardwareKey }
enum class PasswordStrength { Weak, Fair, Strong }
enum class PasswordHint { TooShort, Common, Repeats, Sequence, OneKind, AddWord }
data class PasswordFeedback(val strength: PasswordStrength, val hints: List<PasswordHint>)
/** retryAfterMs: wait before the next password attempt. The attempt counter is not shown in the UI. */
data class NotesStatus(val setUp: Boolean, val unlocked: Boolean, val autoWipe: Boolean, val failedAttempts: Int,
    val wipeAfter: Int, val retryAfterMs: Long, val guarantee: KeyGuarantee,
    val quickAvailable: Boolean = false, val quickEnabled: Boolean = false)
/** A file kept inside the notes (Р-3): viewable there, never exported. */
data class NoteAttachment(val id: String, val name: String, val size: Long)
/** `attachments` must travel with the note: a note saved without them loses them. */
data class Note(val id: String, val title: String, val body: String, val modifiedMs: Long,
    val attachments: List<NoteAttachment> = emptyList())
data class NoteTab(val id: String, val title: String, val notes: List<Note>)
data class NotesExportReport(val files: ULong, val skippedAttachments: ULong)
data class NotesImportReport(val tabs: ULong, val notes: ULong, val skipped: ULong)
/** Sync pair (B4): `twoWay` false = backup (local → cloud). `conflicts`: paths waiting for a decision. */
data class SyncPair(val id: String, val local: Location, val remote: Location, val twoWay: Boolean,
    val trashRemoteDeletions: Boolean, val conflicts: List<String>)
/** Action kinds 1–9 and report kinds 1–8 as documented on NativeSyncItem in the bridge. */
data class SyncItem(val path: String, val kind: Int)
data class SyncPlanSummary(val actions: List<SyncItem>, val reports: List<SyncItem>)
data class SyncRunResult(val done: Int, val failed: Int, val reports: List<SyncItem>)
data class VaultEntry(val name: String, val folder: Boolean, val size: ULong, val modifiedMs: Long)
/** `damaged`: entries that failed authentication and are not shown. */
data class VaultListing(val entries: List<VaultEntry>, val damaged: Long)
/** Single-file encryption: age only (owner decision 2026-11-14, AES Crypt removed). */
enum class FileCryptoFormat(val extension: String) { Age("age") }
data class ContainerEntry(val name: String, val folder: Boolean, val size: ULong)
data class CryptomatorListing(val entries: List<ContainerEntry>, val ignored: Long)
enum class ArchiveFormat(val extension: String) { Zip("zip"), SevenZip("7z") }
data class ArchiveEntry(val path: String, val folder: Boolean, val size: ULong, val compressed: ULong,
    val encrypted: Boolean = false)
/** `code`: Core's error category, e.g. "Unsupported"; `message`: Core's fixed text. */
data class Diagnostic(val atMs: Long, val code: String, val message: String)
enum class VaultKind { Paranoic, Cryptomator }
/** [location]: the vault root for open; [name]: subfolder name, empty for the chosen folder itself. */
data class FoundVault(val location: Location, val name: String, val kind: VaultKind)
/** [unchecked]: subfolders over the inspection limit (64) that were not looked at. */
data class VaultProbe(val vaults: List<FoundVault>, val unchecked: Long)

sealed interface CoreResult<out T> {
    data class Value<T>(val value: T) : CoreResult<T>
    data class Failure(val fault: CoreFault) : CoreResult<Nothing>
}
enum class CoreFault { Disconnected, Permission, Unavailable, Conflict, InvalidName, OutcomeUnknown, Unsupported, Limit, Changed, Busy,
    PasswordRequired, WrongPassword, WeakPassword, RateLimited, Wiped, NotSetUp, Locked, DeviceKey, Cancelled }

enum class PreviewKind { Unsupported, Pdf, Image, Audio, Video }
data class PreviewInfo(val kind: PreviewKind, val mime: String, val length: ULong)
data class PdfPage(val pages: Int, val index: Int, val png: ByteArray)
data class PdfMatches(val pages: List<Int>, val nextPage: Int?)
/** Blocking, bounded IO for platform decoding threads only. No OS path or URL. */
interface MediaSource {
    val length: Long
    fun readAt(offset: Long, size: Int): ByteArray
    fun close()
}

/** Typed identity from Core, never an absolute OS path assembled by the UI. */
data class Location(val storageId: String, val path: String)
enum class EntryKind { File, Directory, Symlink, Other }
/** bytes is meaningful only when sizeKnown; unknown provider metadata stays visible. */
data class FileItem(val location: Location, val name: String, val kind: EntryKind, val bytes: ULong,
    val modifiedMs: ULong? = null, val sizeKnown: Boolean = true)
/** `hidden`: entries with non-portable or duplicate names, omitted rather than failing the folder. */
data class FolderListing(val items: List<FileItem>, val hidden: Long = 0)
/** androidTree is a presentation restriction from the native grant, not a permission decision. */
data class Space(val id: String, val name: String, val root: Location, val writable: Boolean,
    val detachable: Boolean = false, val androidTree: Boolean = false, val localContainer: Boolean = false,
    val cloud: Boolean = false)
enum class TransferMode { Copy, Move }
enum class NameConflict { Fail, Skip, KeepBoth, Replace }
enum class JobPhase { Queued, Running, Succeeded, Failed, Cancelled, NeedsAttention }
enum class JobOutcome { Unknown, Done, Skipped }
enum class JobKind { CreateFolder, CreateFile, Copy, Move, Delete, Save, Recovery, Rename, Other }
/** Stable Core error category of a finished job; never OS text. */
enum class JobError { Permission, Exists, NotFound, Changed, Limit, Unsupported, Cancelled, Attention, Invalid, Io, Other }
enum class AttentionReason { InterruptedWrite, ReplacementBackup, PartialTree, PreparedPublication, Cryptomator, Other }
data class AttentionItem(val job: JobItem, val reason: AttentionReason, val detail: String,
    val backup: Location? = null, val canRestore: Boolean = false, val canDiscard: Boolean = false,
    val canAccept: Boolean = false)
data class JobItem(
    val id: String,
    val phase: JobPhase,
    val bytesDone: ULong = 0uL,
    val bytesTotal: ULong? = null,
    val canCancel: Boolean = false,
    val kind: JobKind = JobKind.Other,
    val source: Location? = null,
    val target: Location? = null,
    val error: JobError? = null,
    val outcome: JobOutcome = JobOutcome.Unknown,
    val resultTarget: Location? = null,
)
enum class FolderProvider { AndroidTree, DesktopDirectory }
/** An OS selection is only a grant reference, not a registered Core storage.
 * writable is an Android grant hint; null on desktop, where Rust must inspect ACLs.
 * Space.writable comes from the registered Core capabilities, never this hint.
 */
data class PickedFolder(val provider: FolderProvider, val reference: String, val writable: Boolean?)
sealed interface FolderPick {
    data class Selected(val folder: PickedFolder) : FolderPick
    data object Cancelled : FolderPick
    data object Failed : FolderPick
}

/** Explicit disconnected composition root. No fake files, jobs or successful writes. */
object DisconnectedCoreGateway : CoreGateway {
    override val connected = false
    private fun unavailable() = CoreResult.Failure(CoreFault.Disconnected)
    override suspend fun spaces() = unavailable()
    override suspend fun attachFolder(folder: PickedFolder) = unavailable()
    override suspend fun list(folder: Location) = unavailable()
    override suspend fun detach(space: Space) = unavailable()
    override suspend fun createDirectory(parent: Location, name: String, requestId: String) = unavailable()
    override suspend fun copy(source: Location, parent: Location, name: String, requestId: String) = unavailable()
    override suspend fun transfer(source: Location, parent: Location, name: String, mode: TransferMode,
        conflict: NameConflict, requestId: String) = unavailable()
    override suspend fun rename(source: Location, name: String, requestId: String) = unavailable()
    override suspend fun delete(source: Location, recursive: Boolean, requestId: String) = unavailable()
    override suspend fun jobs() = unavailable()
    override suspend fun job(id: String) = unavailable()
    override suspend fun cancel(jobId: String) = unavailable()
    override suspend fun attentionList() = unavailable()
    override suspend fun attentionRestore(jobId: String, requestId: String) = unavailable()
    override suspend fun attentionDiscard(jobId: String, requestId: String) = unavailable()
    override suspend fun attentionReviewed(jobId: String) = unavailable()
    override suspend fun attentionAccept(jobId: String) = unavailable()
    override suspend fun inspect(location: Location) = unavailable()
    override suspend fun textOpen(location: Location) = unavailable()
    override suspend fun textSave(location: Location, version: TextFileVersion, text: String) = unavailable()
    override suspend fun textSaveCopy(parent: Location, name: String, text: String) = unavailable()
    override suspend fun officeOpen(location: Location) = unavailable()
    override suspend fun imagePng(location: Location, side: Int) = unavailable()
    override suspend fun pdfPage(location: Location, page: Int, width: Int) = unavailable()
    override suspend fun pdfSearch(location: Location, query: String, start: Int) = unavailable()
    override fun releasePdf() = Unit
    override suspend fun openMedia(location: Location) = unavailable()
}
