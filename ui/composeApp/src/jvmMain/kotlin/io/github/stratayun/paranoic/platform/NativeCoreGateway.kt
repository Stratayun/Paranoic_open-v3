package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.nativecore.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.io.IOException

/** Why the native runtime did not start; shown with a retry, never as plain "not connected". */
enum class StartupFault { NativeLibrary, GrantStore, Core }
class StartupException(val fault: StartupFault, cause: Throwable) : Exception(fault.name, cause)

/** Separate protected store. Never mix grants into appearance preferences. */
interface NativeGrantStore {
    fun read(): List<NativeGrant>
    fun write(grants: List<NativeGrant>)
}
class NativeCoreGateway private constructor(
    private val runtime: NativeRuntime,
    private val grants: NativeGrantStore,
    private val releaseGrant: (NativeGrant) -> Unit,
    private val quickNotesKey: NotesKeyHost? = null,
    private val spaceLabel: (reference: String) -> String? = { null },
) : CoreGateway, ArchiveGateway {
    @Volatile private var live = true
    override val connected get() = live
    private val attaching = Mutex()
    // Held until the physical native call returns, even if its UI caller was cancelled.
    private val textAccess = Mutex()
    private val archiveAdmissions = ConcurrentHashMap<String, CompletableDeferred<Unit>>()
    private val secretEpoch = java.util.concurrent.atomic.AtomicLong()
    @Volatile private var pendingSecretsLock: CompletableDeferred<Unit>? = null

    companion object {
        /** pdfWorker: absolute path of the out-of-process PDF renderer shipped with the app. */
        /** sealer: device-bound key for Core journal records (paths and names). */
        /** releaseGrant: drop the OS permission of a disconnected folder (Android persistable URI). */
        suspend fun open(stateRoot: String, workspaceRoot: String, pdfiumLibrary: String, pdfWorker: String,
            documents: AndroidDocuments, store: NativeGrantStore, sealer: RecordSealer,
            releaseGrant: (NativeGrant) -> Unit = {}, notesKey: NotesKeyHost? = null, notesQuickKey: NotesKeyHost? = null,
            documentWorkers: DocumentWorkers? = null,
            spaceLabel: (reference: String) -> String? = { null }): NativeCoreGateway {
            var acquired: NativeRuntime? = null
            try {
                return withContext(Dispatchers.IO) {
                    val saved = try { store.read() }
                    catch (error: Exception) { throw StartupException(StartupFault.GrantStore, error) }
                    val runtime = try {
                        NativeRuntime(stateRoot, workspaceRoot, pdfiumLibrary, pdfWorker, saved, documents, sealer)
                    } catch (error: LinkageError) { throw StartupException(StartupFault.NativeLibrary, error) }
                    catch (cancelled: CancellationException) { throw cancelled }
                    catch (error: Exception) { throw StartupException(StartupFault.Core, error) }
                    acquired = runtime
                    try { runtime.documentWorkersConfigure(documentWorkers) }
                    catch (error: Exception) { throw StartupException(StartupFault.Core, error) }
                    // Before any notes call: the same device key must be supplied on every start.
                    try { runtime.notesConfigure(notesKey) }
                    catch (error: Exception) { throw StartupException(StartupFault.Core, error) }
                    try { runtime.notesConfigureQuick(notesQuickKey) }
                    catch (error: Exception) { throw StartupException(StartupFault.Core, error) }
                    NativeCoreGateway(runtime, store, releaseGrant, notesQuickKey, spaceLabel)
                }
            } catch (cancelled: CancellationException) {
                withContext(NonCancellable + Dispatchers.IO) {
                    acquired?.let { try { it.shutdown() } finally { it.destroy() } }
                }
                throw cancelled
            } catch (error: Exception) {
                withContext(NonCancellable + Dispatchers.IO) {
                    acquired?.let { try { it.shutdown() } finally { it.destroy() } }
                }
                throw error
            }
        }
    }
    private suspend fun <T> call(write: Boolean = false, block: () -> T): CoreResult<T> = withContext(Dispatchers.IO) {
        try { CoreResult.Value(block()) }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) { CoreResult.Failure(fault(error, write)) }
    }
    private fun Location.native() = NativeLocation(storageId, path)
    private fun NativeLocation.ui() = Location(storageId, path)
    private fun NativeSpace.ui(androidTree: Boolean = false, localContainer: Boolean = false, cloud: Boolean = false) =
        Space(id, name, root.ui(), writable, detachable, androidTree, localContainer, cloud)
    /** Host label for a grant (e.g. "Внутренняя память" for /storage/emulated/0); the reference itself never reaches common UI. */
    private fun Space.labelled(grant: NativeGrant?): Space =
        grant?.takeUnless { it.androidTree }?.let { spaceLabel(it.reference) }?.let { copy(name = it) } ?: this
    /** Presentation hint mirrors local_only in the bridge; Rust still enforces it. Never expose grant references to common UI. */
    private fun NativeGrant.localContainer(): Boolean = if (androidTree)
        reference.startsWith("content://com.android.externalstorage.documents/") else !reference.startsWith("webdav:")
    private fun NativeJob.ui() = JobItem(id, when (phase.toInt()) {
        1 -> JobPhase.Queued; 2 -> JobPhase.Running; 3 -> JobPhase.Succeeded
        4 -> JobPhase.Failed; 5 -> JobPhase.Cancelled; else -> JobPhase.NeedsAttention
    }, bytesDone, bytesTotal, canCancel,
        when (kind.toInt()) {
            1 -> JobKind.CreateFolder; 2 -> JobKind.CreateFile; 3 -> JobKind.Copy; 4 -> JobKind.Move
            5 -> JobKind.Delete; 6 -> JobKind.Save; 7 -> JobKind.Recovery; 8 -> JobKind.Rename; else -> JobKind.Other
        },
        source?.ui(), target?.ui(),
        error?.let {
            when (it.toInt()) {
                1 -> JobError.Permission; 2 -> JobError.Exists; 3 -> JobError.NotFound; 4 -> JobError.Changed
                5 -> JobError.Limit; 6 -> JobError.Unsupported; 7 -> JobError.Cancelled; 8 -> JobError.Attention
                9 -> JobError.Invalid; 10 -> JobError.Io; else -> JobError.Other
            }
        }, when (outcome.toInt()) { 1 -> JobOutcome.Done; 2 -> JobOutcome.Skipped; else -> JobOutcome.Unknown }, resultTarget?.ui())

    override suspend fun spaces() = call {
        val saved = runtime.grants().associateBy { it.id }
        runtime.spaces().map { space ->
            val grant = saved[space.id]
            space.ui(androidTree = grant?.androidTree == true,
                localContainer = !space.detachable || grant?.localContainer() == true,
                cloud = grant?.reference?.startsWith("webdav:") == true).labelled(grant)
        }
    }
    override suspend fun attachFolder(folder: PickedFolder): CoreResult<Space> = attaching.withLock {
        call(write = true) {
            val saved = grants.read()
            val android = folder.provider == FolderProvider.AndroidTree
            val grant = saved.firstOrNull { it.reference == folder.reference && it.androidTree == android }
                ?: NativeGrant(UUID.randomUUID().toString(), folder.reference, android)
            // Persist identity BEFORE native registration. An uncertain reply must
            // never assign a fresh UUID to the same grant on the next startup.
            val updated = saved.filterNot { it.id == grant.id } + grant
            grants.write(updated)
            runtime.attach(grant).ui(androidTree = android, localContainer = grant.localContainer()).labelled(grant)
        }
    }
    /** Idempotent (У-17): 1. withdraw from Core (refused while jobs use it; nothing
     * changes then); 2. remove from the saved grants; 3. release the OS permission.
     * If step 2 failed earlier, the native side no longer knows the grant
     * ("Invalid") while the store still has it: the retry finishes steps 2–3
     * instead of failing forever, and the permission is never released before
     * the grant is gone from Core and from the store. */
    override suspend fun detach(space: Space): CoreResult<Unit> = attaching.withLock {
        call(write = true) {
            val saved = grants.read()
            val removed = try { runtime.detach(space.id) } catch (error: Exception) {
                if (error.javaClass.simpleName != "Invalid") throw error
                saved.firstOrNull { it.id == space.id } ?: throw error
            }
            grants.write(saved.filterNot { it.id == removed.id })
            // Permission last: a failed release leaves an unused grant, never a broken one.
            try { releaseGrant(removed) } catch (_: Exception) {}
        }
    }
    override suspend fun list(folder: Location) = call {
        val listing = runtime.list(folder.native())
        FolderListing(listing.entries.map {
            FileItem(it.location.ui(), it.name, when (it.kind.toInt()) {
                1 -> EntryKind.File; 2 -> EntryKind.Directory; 3 -> EntryKind.Symlink; else -> EntryKind.Other
            }, it.bytes, it.modifiedMs, it.sizeKnown)
        }, listing.skipped.coerceAtMost(Long.MAX_VALUE.toULong()).toLong())
    }
    override suspend fun createDirectory(parent: Location, name: String, requestId: String) = call(write = true) {
        runtime.createDirectory(parent.native(), name, requestId).ui()
    }
    override suspend fun copy(source: Location, parent: Location, name: String, requestId: String) = call(write = true) {
        runtime.copy(source.native(), parent.native(), name, requestId).ui()
    }
    override suspend fun transfer(source: Location, parent: Location, name: String, mode: TransferMode,
        conflict: NameConflict, requestId: String) = call(write = true) {
        val modeCode: UByte = when (mode) { TransferMode.Copy -> 0u; TransferMode.Move -> 1u }
        val conflictCode: UByte = when (conflict) {
            NameConflict.Fail -> 0u; NameConflict.Skip -> 1u; NameConflict.KeepBoth -> 2u; NameConflict.Replace -> 3u
        }
        runtime.transfer(source.native(), parent.native(), name, modeCode, conflictCode, requestId).ui()
    }
    override suspend fun rename(source: Location, name: String, requestId: String) = call(write = true) {
        runtime.rename(source.native(), name, requestId).ui()
    }
    override suspend fun delete(source: Location, recursive: Boolean, requestId: String) = call(write = true) {
        runtime.delete(source.native(), recursive, requestId).ui()
    }
    override suspend fun jobs() = call { runtime.jobs().map { it.ui() } }
    override suspend fun job(id: String) = call { runtime.job(id)?.ui() }
    override suspend fun cancel(jobId: String) = call(write = true) { runtime.cancel(jobId).ui() }
    override suspend fun attentionList() = call {
        runtime.attentionList().map {
            AttentionItem(it.job.ui(), when (it.reason.toInt()) {
                1 -> AttentionReason.InterruptedWrite; 2 -> AttentionReason.ReplacementBackup
                3 -> AttentionReason.PartialTree; 4 -> AttentionReason.PreparedPublication
                5 -> AttentionReason.Cryptomator; else -> AttentionReason.Other
            }, it.detail, it.backup?.ui(), it.canRestore, it.canDiscard, it.canAccept)
        }
    }
    override suspend fun attentionRestore(jobId: String, requestId: String) = call(write = true) {
        runtime.attentionRestore(jobId, requestId).ui()
    }
    override suspend fun attentionDiscard(jobId: String, requestId: String) = call(write = true) {
        runtime.attentionDiscard(jobId, requestId).ui()
    }
    override suspend fun attentionReviewed(jobId: String) = call(write = true) { runtime.attentionReviewed(jobId) }
    override suspend fun attentionAccept(jobId: String) = call(write = true) { runtime.attentionAccept(jobId) }
    private suspend fun <T> textCall(write: Boolean = false, block: () -> T): CoreResult<T> = call(write) {
        if (!textAccess.tryLock()) throw TextBusy()
        try { block() } finally { textAccess.unlock() }
    }
    private fun NativeFileVersion.textUi() = TextFileVersion(bytes, modifiedMs, blake3)
    override suspend fun textOpen(location: Location): CoreResult<TextDocument> = textCall {
        val opened = runtime.textOpen(location.native())
        TextDocument(opened.text, when (opened.encoding.toInt()) {
            1 -> TextEncoding.Utf8; 2 -> TextEncoding.Utf8Bom
            3 -> TextEncoding.Utf16Le; 4 -> TextEncoding.Utf16Be
            else -> throw UnsupportedOperationException()
        }, opened.version.textUi())
    }
    override suspend fun textSave(location: Location, version: TextFileVersion, text: String) = textCall(write = true) {
        runtime.textSave(location.native(), NativeFileVersion(version.bytes, version.modifiedMs, version.blake3), text).textUi()
    }
    override suspend fun textSaveCopy(parent: Location, name: String, text: String) = textCall(write = true) {
        runtime.textSaveAs(parent.native(), name, text).textUi()
    }
    override suspend fun textPagerOpen(location: Location): CoreResult<TextPager> {
        var acquired: String? = null
        var delivered = false
        try {
            val result = call {
                val opened = runtime.textPagerOpen(location.native())
                acquired = opened.handle
                if (opened.rows > Long.MAX_VALUE.toULong()) throw UnsupportedOperationException()
                // This export uses 0..3; text_open uses 1..4. Keep the two
                // native encodings explicit rather than silently shifting them.
                val encoding = when (opened.encoding.toInt()) {
                    0 -> TextEncoding.Utf8; 1 -> TextEncoding.Utf8Bom
                    2 -> TextEncoding.Utf16Le; 3 -> TextEncoding.Utf16Be
                    else -> throw UnsupportedOperationException()
                }
                TextPager(opened.handle, opened.rows.toLong(), encoding, opened.bytes)
            }
            currentCoroutineContext().ensureActive()
            delivered = result is CoreResult.Value
            return result
        } finally {
            // withContext can discard a successful native reply on cancellation.
            // The acquired handle still belongs to this call until delivery.
            if (!delivered) acquired?.let { handle ->
                withContext(NonCancellable + Dispatchers.IO) {
                    try { runtime.textPagerClose(handle) } catch (_: Exception) {}
                }
            }
        }
    }
    override suspend fun textPagerRead(handle: String, first: Long, count: Int): CoreResult<List<String>> {
        if (first < 0 || count !in 1..2000) return CoreResult.Failure(CoreFault.InvalidName)
        return call { runtime.textPagerRead(handle, first.toULong(), count.toUInt()) }
    }
    override suspend fun textPagerClose(handle: String): CoreResult<Unit> = withContext(NonCancellable) {
        call { runtime.textPagerClose(handle) }
    }
    override suspend fun passwordFeedback(password: String) = call {
        val result = runtime.passwordFeedback(password)
        val strength = when (result.strength.toInt()) {
            0 -> PasswordStrength.Weak; 1 -> PasswordStrength.Fair; 2 -> PasswordStrength.Strong
            else -> throw UnsupportedOperationException()
        }
        PasswordFeedback(strength, result.hints.toList().mapNotNull { hint -> when (hint.toInt()) {
            1 -> PasswordHint.TooShort; 2 -> PasswordHint.Common; 3 -> PasswordHint.Repeats
            4 -> PasswordHint.Sequence; 5 -> PasswordHint.OneKind; 6 -> PasswordHint.AddWord; else -> null
        } }.distinct())
    }
    override suspend fun generatePassphrase(words: Int): CoreResult<String> {
        if (words !in 4..10) return CoreResult.Failure(CoreFault.InvalidName)
        return call { runtime.generatePassphrase(words.toUByte()) }
    }
    override suspend fun vaultRecoveryChallenge(count: Int): CoreResult<List<Int>> {
        if (count !in 2..3) return CoreResult.Failure(CoreFault.InvalidName)
        return call {
            val indices = runtime.vaultRecoveryChallenge(count.toUByte())
            if (indices.size != count || indices.distinct().size != count || indices.any { it > Int.MAX_VALUE.toUInt() })
                throw UnsupportedOperationException()
            indices.map { it.toInt() }
        }
    }
    override suspend fun vaultRecoveryCheck(recovery: String, indices: List<Int>, answers: List<String>): CoreResult<Boolean> {
        if (indices.size !in 2..3 || indices.size != answers.size || indices.distinct().size != indices.size || indices.any { it < 0 })
            return CoreResult.Failure(CoreFault.InvalidName)
        return call { runtime.vaultRecoveryCheck(recovery, indices.map { it.toUInt() }, answers) }
    }
    override suspend fun vaultResetPassword(handle: String, recovery: String, newPassword: String) = protectedCall(write = true) {
        runtime.vaultResetPassword(handle, recovery, newPassword)
    }
    override suspend fun officeOpen(location: Location): CoreResult<OfficeDocument> = call {
        runtime.officeOpen(location.native()).document()
    }
    private fun NativeOfficeView.document(): OfficeDocument = when (kind.toInt()) {
        1 -> OfficeDocument.Word(blocks.map { block ->
            when (block.kind.toInt()) {
                1 -> OfficeBlock.Paragraph(block.heading?.toInt(), block.list,
                    block.runs.map { OfficeRun(it.text, it.bold, it.italic, it.underline) })
                2 -> OfficeBlock.Table(block.rows)
                else -> throw UnsupportedOperationException()
            }
        }, truncated)
        2 -> OfficeDocument.Workbook(sheets.map { OfficeSheet(it.name, it.rows, it.truncated) }, truncated)
        else -> throw UnsupportedOperationException()
    }
    override suspend fun inspect(location: Location) = call {
        val p = runtime.inspect(location.native())
        PreviewInfo(when (p.kind.toInt()) {
            1 -> PreviewKind.Pdf; 2 -> PreviewKind.Image; 3 -> PreviewKind.Audio; 4 -> PreviewKind.Video
            else -> PreviewKind.Unsupported
        }, p.mime, p.length)
    }
    override suspend fun imagePng(location: Location, side: Int) = call { runtime.imagePng(location.native(), side.toUInt()) }
    override suspend fun pdfPage(location: Location, page: Int, width: Int) = call {
        val p = runtime.pdfPage(location.native(), page.toUInt(), width.toUInt())
        PdfPage(p.pages.toInt(), p.index.toInt(), p.png)
    }
    override suspend fun pdfSearch(location: Location, query: String, start: Int) = call {
        val p = runtime.pdfSearch(location.native(), query, start.toUInt())
        PdfMatches(p.pages.map { it.toInt() }, p.nextPage?.toInt())
    }
    override fun releasePdf() {
        if (!live) return
        // Killing the worker is quick but still IO: keep it off the UI thread.
        Thread { try { runtime.closePdf() } catch (_: Exception) {} }.apply { isDaemon = true; name = "paranoic-pdf-release" }.start()
    }
    override suspend fun openMedia(location: Location): CoreResult<MediaSource> {
        var acquired: MediaSource? = null
        try {
            return call {
                val reader = runtime.openStream(location.native())
                try {
                    val size = reader.length()
                    if (size > Long.MAX_VALUE.toULong()) throw IOException("media size limit")
                    object : MediaSource {
                        private val closed = java.util.concurrent.atomic.AtomicBoolean(false)
                        override val length = size.toLong()
                        override fun readAt(offset: Long, size: Int): ByteArray {
                            if (closed.get()) throw IOException("closed media")
                            if (offset < 0 || size !in 0..1048576) throw IOException("invalid media read")
                            return reader.readAt(offset.toULong(), size.toUInt())
                        }
                        override fun close() {
                            if (closed.compareAndSet(false, true)) try { reader.releaseStream() } finally { reader.destroy() }
                        }
                    }.also { acquired = it }
                } catch (error: Exception) {
                    try { reader.releaseStream() } finally { reader.destroy() }
                    throw error
                }
            }
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable + Dispatchers.IO) { acquired?.close() }
            throw cancelled
        }
    }
    override suspend fun attachWebDav(url: String, user: String, token: String): CoreResult<Space> = attaching.withLock {
        call(write = true) {
            val attached = runtime.attachWebdav(url, user, token)
            // Persisted only after it worked; a failed save withdraws the live connection.
            try { grants.write(grants.read().filterNot { it.id == attached.grant.id } + attached.grant) }
            catch (error: Exception) { try { runtime.detach(attached.grant.id) } catch (_: Exception) {}; throw error }
            attached.space.ui(cloud = true)
        }
    }
    override suspend fun reconnectCloud() = call { runtime.reconnectCloud().map { it.ui(cloud = true) } }
    override fun lockSecrets() {
        if (!live) return
        secretEpoch.incrementAndGet()
        incomingReads.forEach { try { it.cancel() } catch (_: Exception) {} }
        val done = CompletableDeferred<Unit>()
        pendingSecretsLock = done
        Thread {
            try { runtime.lockAllSecrets(); done.complete(Unit) }
            catch (error: Exception) { done.completeExceptionally(error) }
        }.apply { isDaemon = true; name = "paranoic-quick-lock" }.start()
    }

    /** Resume cannot authenticate before the background lock has finished. */
    private suspend fun <T> secretCall(write: Boolean = false, block: () -> T): CoreResult<T> {
        val epoch = secretEpoch.get()
        try { pendingSecretsLock?.await() }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { return CoreResult.Failure(CoreFault.Locked) }
        return call(write) {
            if (epoch != secretEpoch.get()) throw SecretRevoked()
            val value = block()
            if (epoch != secretEpoch.get()) {
                runtime.lockAllSecrets()
                throw SecretRevoked()
            }
            value
        }
    }

    private val protectedAccess = Mutex()
    /** Reconciliation/unlock waits for actual completion of an earlier notes or
     * personal-folder call, including a cancelled dispatcher handoff. Quick Lock
     * bypasses this mutex to revoke keys immediately. */
    private suspend fun <T> protectedCall(write: Boolean = false, block: () -> T): CoreResult<T> =
        protectedAccess.withLock { secretCall(write, block) }
    override suspend fun awaitProtectedWrites() = protectedCall { Unit }
    private fun guarantee(code: UByte) = when (code.toInt()) { 2 -> KeyGuarantee.HardwareKey; 1 -> KeyGuarantee.DeviceKey; else -> KeyGuarantee.PasswordOnly }
    override suspend fun notesStatus() = protectedCall {
        val s = runtime.notesStatus()
        NotesStatus(s.setUp, s.unlocked, s.autoWipe, s.failedAttempts.toInt(), s.wipeAfter.toInt(),
            s.retryAfterMs.coerceAtMost(Long.MAX_VALUE.toULong()).toLong(), guarantee(s.guarantee), s.quickAvailable, s.quickEnabled)
    }
    override suspend fun notesPasswordStrength(password: String) = call {
        when (runtime.notesPasswordStrength(password).toInt()) { 2 -> PasswordStrength.Strong; 1 -> PasswordStrength.Fair; else -> PasswordStrength.Weak }
    }
    override suspend fun notesCreate(password: String) = protectedCall(write = true) { runtime.notesCreate(password) }
    override suspend fun notesUnlock(password: String) = protectedCall { runtime.notesUnlock(password) }
    override suspend fun notesQuickEnable() = protectedCall(write = true) { runtime.notesQuickEnable() }
    override suspend fun notesQuickDisable() = protectedCall(write = true) {
        runtime.notesQuickDisable()
        // Core removes the live wrappers; deleting the host keys also revokes old wrapper copies.
        if (quickNotesKey?.eraseKey()?.let { it != 0u } == true) throw QuickKeyUnavailable()
    }
    override suspend fun notesQuickUnlock() = protectedCall { runtime.notesQuickUnlock() }
    override suspend fun notesLock() = protectedCall { runtime.notesLock(); runtime.lockAllSecrets() }
    override suspend fun notesRead() = protectedCall {
        runtime.notesRead().tabs.map { t -> NoteTab(t.id, t.title, t.notes.map { n ->
            Note(n.id, n.title, n.body, n.modifiedMs.toLong(), n.attachments.map { NoteAttachment(it.id, it.name, it.size.toLong()) })
        }) }
    }
    override suspend fun notesSave(tabs: List<NoteTab>) = protectedCall(write = true) {
        runtime.notesSave(NativeNotebook(tabs.map { t ->
            NativeTab(t.id, t.title, t.notes.map { n -> NativeNote(n.id, n.title, n.body, n.modifiedMs.coerceAtLeast(0).toULong(),
                n.attachments.map { NativeAttachment(it.id, it.name, it.size.coerceAtLeast(0).toULong()) }) })
        }))
    }
    override suspend fun notesChangePassword(old: String, new: String) = protectedCall(write = true) { runtime.notesChangePassword(old, new) }
    override suspend fun notesExport(parent: Location, name: String) = protectedCall(write = true) {
        runtime.notesExport(parent.native(), name).let { NotesExportReport(it.files, it.skippedAttachments) }
    }
    override suspend fun notesImport(sources: List<Location>, rootTab: String, intoTab: String?): CoreResult<NotesImportReport> {
        if (sources.isEmpty() || sources.size > 128 || rootTab.isBlank() || rootTab.length > 128)
            return CoreResult.Failure(CoreFault.InvalidName)
        return protectedCall(write = true) {
            runtime.notesImport(sources.map { it.native() }, rootTab, intoTab).let { NotesImportReport(it.tabs, it.notes, it.skipped) }
        }
    }
    override suspend fun notesSetDuress(password: String?) = protectedCall(write = true) { runtime.notesSetDuress(password) }
    override suspend fun notesDuressEnabled() = protectedCall { runtime.notesDuressEnabled() }
    override suspend fun safeOpen(): CoreResult<String> = protectedAccess.withLock {
        openedFolder(write = true, release = { runtime.vaultLock(it) }) { runtime.safeOpen() }
    }
    override suspend fun safeClear() = protectedCall(write = true) { guarantee(runtime.safeClear()) }
    private fun NativeSyncPair.ui() = SyncPair(id, local.ui(), remote.ui(), mode.toInt() == 2, trashRemoteDeletions, conflicts)
    private fun NativeSyncItem.ui() = SyncItem(path, kind.toInt())
    override suspend fun syncPairs() = call { runtime.syncPairs().map { it.ui() } }
    override suspend fun syncPairAdd(local: Location, remote: Location, twoWay: Boolean, trashRemoteDeletions: Boolean) =
        call(write = true) { runtime.syncPairAdd(local.native(), remote.native(), if (twoWay) 2u else 1u, trashRemoteDeletions).ui() }
    override suspend fun syncPairRemove(id: String) = call(write = true) { runtime.syncPairRemove(id) }
    override suspend fun syncPlan(id: String) = call(write = true) {
        runtime.syncPlan(id).let { p -> SyncPlanSummary(p.actions.map { it.ui() }, p.reports.map { it.ui() }) }
    }
    override suspend fun syncRun(id: String) = call(write = true) {
        runtime.syncRun(id).let { r -> SyncRunResult(r.done.toInt(), r.failed.toInt(), r.reports.map { it.ui() }) }
    }
    override suspend fun syncResolve(id: String, path: String, keepLocal: Boolean) = call(write = true) { runtime.syncResolve(id, path, keepLocal) }
    override suspend fun syncForget(id: String, path: String) = call(write = true) { runtime.syncForget(id, path) }
    override suspend fun notesAttachmentAdd(name: String, data: ByteArray) = protectedCall(write = true) {
        try { runtime.notesAttachmentAdd(name, data).let { NoteAttachment(it.id, it.name, it.size.toLong()) } } finally { data.fill(0) }
    }
    override suspend fun notesAttachmentRead(id: String): CoreResult<ByteArray> {
        var bytes: ByteArray? = null
        var delivered = false
        try {
            val result = protectedCall { runtime.notesAttachmentRead(id).also { bytes = it } }
            currentCoroutineContext().ensureActive()
            delivered = result is CoreResult.Value
            return result
        } finally {
            // protectedCall may reject a late reply after lock, or dispatcher handoff may be cancelled.
            if (!delivered) bytes?.fill(0)
        }
    }
    override suspend fun notesSetAutoWipe(enabled: Boolean) = protectedCall(write = true) { runtime.notesSetAutoWipe(enabled) }
    override suspend fun notesClear() = protectedCall(write = true) { guarantee(runtime.notesClear()) }

    private fun FileCryptoFormat.native(): UByte = when (this) { FileCryptoFormat.Age -> 1u }
    override suspend fun encryptFile(source: Location, parent: Location, name: String, format: FileCryptoFormat, password: String) =
        call(write = true) { runtime.encryptFile(source.native(), parent.native(), name, format.native(), password) }
    override suspend fun decryptFile(source: Location, parent: Location, name: String, format: FileCryptoFormat, password: String) =
        call(write = true) { runtime.decryptFile(source.native(), parent.native(), name, format.native(), password) }

    override suspend fun vaultCreate(parent: Location, name: String, password: String) =
        protectedAccess.withLock { openedFolder(write = true, release = { runtime.vaultLock(it) }) { runtime.vaultCreate(parent.native(), name, password) } }
    override suspend fun vaultOpen(root: Location, password: String) =
        protectedAccess.withLock { openedFolder(write = false, release = { runtime.vaultLock(it) }) { runtime.vaultOpen(root.native(), password) } }
    override suspend fun vaultLock(handle: String) = call { runtime.vaultLock(handle) }
    override suspend fun vaultList(handle: String, path: List<String>) = protectedCall {
        val l = runtime.vaultList(handle, path)
        VaultListing(l.entries.map { VaultEntry(it.name, it.folder, it.size, it.modifiedMs.coerceAtMost(Long.MAX_VALUE.toULong()).toLong()) },
            l.damaged.coerceAtMost(Long.MAX_VALUE.toULong()).toLong())
    }
    override suspend fun vaultAdd(handle: String, path: List<String>, source: Location) = protectedCall(write = true) { runtime.vaultAdd(handle, path, source.native()) }
    override suspend fun vaultMoveIn(handle: String, path: List<String>, source: Location, requestId: String) = protectedCall(write = true) {
        val result = runtime.vaultMoveIn(handle, path, source.native(), requestId)
        VaultMoveInReport(result.bytes, result.sourceRemoved)
    }
    private val incomingReads = ConcurrentHashMap.newKeySet<IncomingByteSource>()
    override suspend fun vaultAddIncoming(handle: String, path: List<String>, name: String, document: IncomingDocument): CoreResult<Unit> {
        val epoch = secretEpoch.get()
        return try { protectedCall(write = true) {
            if (epoch != secretEpoch.get()) throw SecretRevoked()
            val source = document.open()
            incomingReads += source
            try {
                if (epoch != secretEpoch.get()) { source.cancel(); throw SecretRevoked() }
                runtime.vaultAddStream(handle, path, name, document.size, document.modifiedMs, object : IncomingStream {
                    override fun read(max: UInt): HostBytes = try {
                        if (max == 0u || max > 1_048_576u || epoch != secretEpoch.get()) HostBytes(3u, byteArrayOf())
                        else HostBytes(0u, source.read(max.toInt()))
                    } catch (_: SecurityException) { HostBytes(1u, byteArrayOf()) }
                    catch (_: Exception) { HostBytes(3u, byteArrayOf()) }
                })
            } finally { incomingReads -= source; source.close() }
        } } finally { document.discard() }
    }
    override suspend fun vaultOpenStream(handle: String, path: List<String>, name: String) =
        openProtectedStream { runtime.vaultStreamOpen(handle, path, name) }
    override suspend fun cryptomatorOpenStream(handle: String, path: List<String>, name: String) =
        openProtectedStream { runtime.cryptomatorStreamOpen(handle, path, name) }
    private suspend fun openProtectedStream(open: () -> NativeVaultStream): CoreResult<MediaSource> {
        var owned: MediaSource? = null
        var delivered = false
        val epoch = secretEpoch.get()
        try {
            val result = protectedCall {
                if (epoch != secretEpoch.get()) throw SecretRevoked()
                val opened = open()
                try {
                    if (opened.size > Long.MAX_VALUE.toULong()) throw UnsupportedOperationException()
                    object : MediaSource {
                        private val closed = java.util.concurrent.atomic.AtomicBoolean(false)
                        override val length = opened.size.toLong()
                        override fun readAt(offset: Long, size: Int): ByteArray {
                            if (closed.get() || epoch != secretEpoch.get() || offset < 0 || size !in 0..1_048_576)
                                throw IOException("closed or invalid protected stream")
                            val bytes = runtime.vaultStreamRead(opened.id, offset.toULong(), size.toUInt())
                            if (closed.get() || epoch != secretEpoch.get()) { bytes.fill(0); throw IOException("protected stream revoked") }
                            return bytes
                        }
                        override fun close() {
                            if (closed.compareAndSet(false, true)) runtime.vaultStreamClose(opened.id)
                        }
                    }.also { owned = it }
                } catch (error: Exception) { runtime.vaultStreamClose(opened.id); throw error }
            }
            currentCoroutineContext().ensureActive()
            delivered = result is CoreResult.Value
            return result
        } finally {
            if (!delivered) withContext(NonCancellable + Dispatchers.IO) { try { owned?.close() } catch (_: Exception) {} }
        }
    }
    override suspend fun vaultTextOpen(handle: String, path: List<String>, name: String) = protectedCall {
        runtime.vaultTextOpen(handle, path, name).document()
    }
    override suspend fun vaultPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int) =
        protectedPdfPage { runtime.vaultPdfPage(handle, path, name, page.toUInt(), width.toUInt()) }
    override suspend fun cryptomatorPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int) =
        protectedPdfPage { runtime.cryptomatorPdfPage(handle, path, name, page.toUInt(), width.toUInt()) }
    private suspend fun protectedPdfPage(render: () -> NativePdfPage): CoreResult<PdfPage> {
        var owned: ByteArray? = null
        var delivered = false
        try {
            val result = protectedCall {
                val page = render()
                owned = page.png
                PdfPage(page.pages.toInt(), page.index.toInt(), page.png)
            }
            currentCoroutineContext().ensureActive()
            delivered = result is CoreResult.Value
            return result
        } finally { if (!delivered) owned?.fill(0) }
    }
    override suspend fun vaultPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int) = protectedCall {
        runtime.vaultPdfSearch(handle, path, name, query, start.toUInt()).let { PdfMatches(it.pages.map { p -> p.toInt() }, it.nextPage?.toInt()) }
    }
    override suspend fun cryptomatorPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int) = protectedCall {
        runtime.cryptomatorPdfSearch(handle, path, name, query, start.toUInt()).let { PdfMatches(it.pages.map { p -> p.toInt() }, it.nextPage?.toInt()) }
    }
    override suspend fun vaultOfficeOpen(handle: String, path: List<String>, name: String) = protectedCall {
        runtime.vaultOfficeOpen(handle, path, name).document()
    }
    override suspend fun cryptomatorOfficeOpen(handle: String, path: List<String>, name: String) = protectedCall {
        runtime.cryptomatorOfficeOpen(handle, path, name).document()
    }
    private fun NativeVaultText.document(): VaultTextDocument {
        val encoding = when (this.encoding.toInt()) {
            1 -> TextEncoding.Utf8; 2 -> TextEncoding.Utf8Bom; 3 -> TextEncoding.Utf16Le; 4 -> TextEncoding.Utf16Be
            else -> throw UnsupportedOperationException()
        }
        return VaultTextDocument(text, encoding, VaultTextVersion(versionBytes, versionModifiedMs, versionToken))
    }
    override suspend fun vaultTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String) = protectedCall(write = true) {
        runtime.vaultTextSave(handle, path, name, version.token, text)
    }
    override suspend fun vaultTextCreate(handle: String, path: List<String>, name: String, text: String) = protectedCall(write = true) {
        runtime.vaultTextCreate(handle, path, name, text)
    }
    override suspend fun vaultArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?) = protectedCall {
        runtime.vaultArchiveList(handle, path, name, format.native(), password).map { ArchiveEntry(it.path, it.folder, it.size, it.compressed, it.encrypted) }
    }
    override suspend fun vaultArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?, targetPath: List<String>, targetName: String) = protectedCall(write = true) {
        runtime.vaultArchiveExtract(handle, path, name, format.native(), password, targetPath, targetName)
    }
    override suspend fun cryptomatorTextOpen(handle: String, path: List<String>, name: String) = protectedCall {
        runtime.cryptomatorTextOpen(handle, path, name).document()
    }
    override suspend fun cryptomatorTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String) = protectedCall(write = true) {
        runtime.cryptomatorTextSave(handle, path, name, version.token, text)
    }
    override suspend fun cryptomatorTextCreate(handle: String, path: List<String>, name: String, text: String) = protectedCall(write = true) {
        runtime.cryptomatorTextCreate(handle, path, name, text)
    }
    override suspend fun cryptomatorArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?) = protectedCall {
        runtime.cryptomatorArchiveList(handle, path, name, format.native(), password).map { ArchiveEntry(it.path, it.folder, it.size, it.compressed, it.encrypted) }
    }
    override suspend fun cryptomatorArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?, targetPath: List<String>, targetName: String) = protectedCall(write = true) {
        runtime.cryptomatorArchiveExtract(handle, path, name, format.native(), password, targetPath, targetName)
    }
    override suspend fun vaultExport(handle: String, path: List<String>, name: String, targetParent: Location) =
        protectedCall(write = true) { runtime.vaultExport(handle, path, name, targetParent.native()) }
    override suspend fun vaultMkdir(handle: String, path: List<String>, name: String) = protectedCall(write = true) { runtime.vaultMkdir(handle, path, name) }
    override suspend fun vaultDelete(handle: String, path: List<String>, name: String, folder: Boolean) =
        protectedCall(write = true) { runtime.vaultDelete(handle, path, name, folder) }
    override suspend fun vaultRename(handle: String, path: List<String>, name: String, newName: String, folder: Boolean) =
        protectedCall(write = true) { runtime.vaultRename(handle, path, name, newName, folder) }
    override suspend fun vaultMove(handle: String, path: List<String>, name: String, folder: Boolean, targetPath: List<String>) =
        protectedCall(write = true) { runtime.vaultMove(handle, path, name, folder, targetPath) }
    override suspend fun vaultReplace(handle: String, path: List<String>, name: String, source: Location) =
        protectedCall(write = true) { runtime.vaultReplace(handle, path, name, source.native()) }
    override suspend fun vaultChangePassword(handle: String, oldPassword: String, newPassword: String) =
        protectedCall(write = true) { runtime.vaultChangePassword(handle, oldPassword, newPassword) }
    override suspend fun vaultSetRecovery(handle: String, password: String) =
        protectedCall(write = true) { runtime.vaultSetRecovery(handle, password) }
    override suspend fun vaultOpenRecovery(root: Location, recovery: String) =
        protectedAccess.withLock { openedFolder(write = false, release = { runtime.vaultLock(it) }) { runtime.vaultOpenRecovery(root.native(), recovery) } }

    /** Acquire off-main, and release a handle if cancellation prevents the UI from adopting it. */
    private suspend fun openedFolder(write: Boolean, release: (String) -> Unit = { runtime.cryptomatorLock(it) }, open: () -> String): CoreResult<String> {
        var acquired: String? = null
        try { return secretCall(write) { open().also { acquired = it } } }
        catch (cancelled: CancellationException) {
            withContext(NonCancellable + Dispatchers.IO) { acquired?.let { try { release(it) } catch (_: Exception) {} } }
            throw cancelled
        }
    }
    override suspend fun cryptomatorCreate(parent: Location, name: String, password: String) =
        openedFolder(write = true) { runtime.cryptomatorCreate(parent.native(), name, password) }
    override suspend fun cryptomatorOpen(root: Location, password: String) =
        openedFolder(write = false) { runtime.cryptomatorOpen(root.native(), password) }
    override suspend fun cryptomatorList(handle: String, path: List<String>) = secretCall {
        val listing = runtime.cryptomatorList(handle, path)
        CryptomatorListing(listing.entries.map { ContainerEntry(it.name, it.folder, it.size) },
            listing.ignored.coerceAtMost(Long.MAX_VALUE.toULong()).toLong())
    }
    override suspend fun cryptomatorAdd(handle: String, path: List<String>, source: Location) =
        secretCall(write = true) { runtime.cryptomatorAdd(handle, path, source.native()) }
    override suspend fun cryptomatorExport(handle: String, path: List<String>, name: String, parent: Location) =
        secretCall(write = true) { runtime.cryptomatorExport(handle, path, name, parent.native()) }
    override suspend fun cryptomatorMkdir(handle: String, path: List<String>, name: String) =
        secretCall(write = true) { runtime.cryptomatorMkdir(handle, path, name) }
    override suspend fun cryptomatorRename(handle: String, path: List<String>, name: String, newName: String) =
        secretCall(write = true) { runtime.cryptomatorRename(handle, path, name, newName) }
    override suspend fun cryptomatorDelete(handle: String, path: List<String>, name: String) =
        secretCall(write = true) { runtime.cryptomatorDelete(handle, path, name) }
    override suspend fun cryptomatorChangePassword(handle: String, oldPassword: String, newPassword: String) =
        secretCall(write = true) { runtime.cryptomatorChangePassword(handle, oldPassword, newPassword) }
    override suspend fun cryptomatorLock(handle: String) = call { runtime.cryptomatorLock(handle) }
    private fun ArchiveFormat.native(): UByte = when (this) { ArchiveFormat.Zip -> 1u; ArchiveFormat.SevenZip -> 2u }

    /** A cancellable await observes parent cancellation even while the IO child
     * is inside a synchronous native call. Admission completes before cancel. */
    private suspend fun <T> archiveCall(requestId: String, write: Boolean = false, block: () -> T): CoreResult<T> = coroutineScope {
        val admitted = CompletableDeferred<Unit>()
        if (archiveAdmissions.putIfAbsent(requestId, admitted) != null) return@coroutineScope CoreResult.Failure(CoreFault.Busy)
        val task = async(Dispatchers.IO) {
            try {
                runtime.archiveReservePreparation(requestId)
                admitted.complete(Unit)
                currentCoroutineContext().ensureActive()
                CoreResult.Value(block())
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { CoreResult.Failure(fault(error, write)) }
            finally {
                admitted.complete(Unit)
                try { runtime.archiveReleasePreparation(requestId) } catch (_: Exception) {}
            }
        }
        task.invokeOnCompletion { admitted.complete(Unit) } // also if cancelled before dispatch
        try { task.await() }
        catch (cancelled: CancellationException) {
            withContext(NonCancellable + Dispatchers.IO) {
                admitted.await()
                try { runtime.archiveCancelPreparation(requestId) } catch (_: Exception) {}
            }
            throw cancelled
        } finally { archiveAdmissions.remove(requestId, admitted) }
    }
    override suspend fun archiveList(source: Location, format: ArchiveFormat, password: String?) =
        archiveListWithRequest(source, format, password, UUID.randomUUID().toString())
    override suspend fun archiveExtract(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String) =
        archiveExtractWithRequest(source, format, password, parent, name, UUID.randomUUID().toString())
    override suspend fun archiveCreate(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String) =
        archiveCreateWithRequest(sources, format, password, parent, name, UUID.randomUUID().toString())
    override suspend fun archiveListWithRequest(source: Location, format: ArchiveFormat, password: String?, requestId: String) =
        archiveCall(requestId) { runtime.archiveListWithRequest(source.native(), format.native(), password, requestId)
            .map { ArchiveEntry(it.path, it.folder, it.size, it.compressed, it.encrypted) } }
    override suspend fun archiveExtractWithRequest(source: Location, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String) =
        archiveCall(requestId, write = true) { runtime.archiveExtractWithRequest(source.native(), format.native(), password, parent.native(), name, requestId) }
    override suspend fun archiveCreateWithRequest(sources: List<Location>, format: ArchiveFormat, password: String?, parent: Location, name: String, requestId: String) =
        archiveCall(requestId, write = true) { runtime.archiveCreateWithRequest(sources.map { it.native() }, format.native(), password, parent.native(), name, requestId) }
    override suspend fun diagnostics() = call { runtime.diagnosticsRecent().map { Diagnostic(it.atMs.toLong(), it.code, it.message) } }
    override suspend fun diagnosticsReportText() = call { runtime.diagnosticsReportText() }
    override suspend fun clearDiagnostics() = call { runtime.diagnosticsClear() }
    override suspend fun vaultAnchorsDamaged() = (call { runtime.vaultAnchorsDamaged() } as? CoreResult.Value)?.value == true
    override suspend fun vaultAnchorsReset() = call { runtime.vaultAnchorsReset() }
    override suspend fun trashSupported(space: Space) = (call { runtime.trashSupported(space.root.storageId) } as? CoreResult.Value)?.value == true
    override suspend fun trashPut(source: Location) = call { runtime.trashPut(source.native()) }
    override suspend fun trashList(space: Space) = call {
        runtime.trashList(space.root.storageId).map { TrashItem(it.bucket, it.name, it.originalFolder, it.deletedMs.toLong(), it.folder, it.bytes.toLong()) }
    }
    override suspend fun trashRestore(space: Space, bucket: String) = call { runtime.trashRestore(space.root.storageId, bucket) }
    override suspend fun trashPurge(space: Space, bucket: String) = call { runtime.trashPurge(space.root.storageId, bucket) }
    override suspend fun trashEmpty(space: Space) = call { runtime.trashEmpty(space.root.storageId).toInt() }
    override suspend fun renameProbe(space: Space) = call {
        when (runtime.renameProbe(space.id).toInt()) {
            0 -> RenameProbe.Supported; 1 -> RenameProbe.Unsupported; 2 -> RenameProbe.Ignored
            3 -> RenameProbe.Failed; else -> RenameProbe.NotLocal
        }
    }
    override suspend fun vaultProbe(folder: Location) = call {
        val probe = runtime.vaultProbe(folder.native())
        VaultProbe(probe.vaults.mapNotNull { found ->
            val kind = when (found.kind.toInt()) { 1 -> VaultKind.Paranoic; 2 -> VaultKind.Cryptomator; else -> null }
            kind?.let { FoundVault(found.location.ui(), found.name, it) }
        }, probe.unchecked.toLong())
    }
    override suspend fun cancelArchiveRequest(requestId: String): CoreResult<Unit> {
        archiveAdmissions[requestId]?.await()
        return call { runtime.archiveCancelPreparation(requestId) }
    }

    suspend fun shutdown() = withContext(Dispatchers.IO) {
        if (live) { live = false; try { runtime.shutdown() } finally { runtime.destroy() } }
    }
}
private class TextBusy : Exception()
private class SecretRevoked : Exception()
private class QuickKeyUnavailable : Exception()
private fun fault(error: Exception, write: Boolean): CoreFault = if (error is TextBusy) CoreFault.Busy
    else if (error is SecretRevoked) CoreFault.Locked
    else if (error is QuickKeyUnavailable) CoreFault.DeviceKey
    else if (error is UnsupportedOperationException) CoreFault.Unsupported else when (error.javaClass.simpleName) {
    "Denied" -> CoreFault.Permission
    "Unsupported" -> CoreFault.Unsupported
    "Invalid" -> CoreFault.InvalidName
    "Conflict" -> CoreFault.Conflict
    "Limit" -> CoreFault.Limit
    "Changed" -> CoreFault.Changed
    "Attention" -> CoreFault.OutcomeUnknown
    "Busy" -> CoreFault.Busy
    "Cancelled" -> CoreFault.Cancelled
    "PasswordRequired" -> CoreFault.PasswordRequired
    "WrongPassword" -> CoreFault.WrongPassword
    "WeakPassword" -> CoreFault.WeakPassword
    "RateLimited" -> CoreFault.RateLimited
    "Wiped" -> CoreFault.Wiped
    "NotSetUp" -> CoreFault.NotSetUp
    "Locked" -> CoreFault.Locked
    "DeviceKey" -> CoreFault.DeviceKey
    else -> if (write) CoreFault.OutcomeUnknown else CoreFault.Unavailable
}

/** Native Unix/Windows grants do not use Android callbacks. */
object NoAndroidDocuments : AndroidDocuments {
    private fun info() = HostInfo(2u, 0u, 0uL, null)
    override fun metadata(tree: String, path: String) = info()
    override fun list(tree: String, path: String, limit: UInt) = HostListing(2u, emptyList())
    override fun open(tree: String, path: String) = HostOpened(2u, 0uL, info())
    override fun read(handle: ULong, offset: ULong, size: UInt) = HostBytes(2u, byteArrayOf())
    override fun fileMetadata(handle: ULong) = info()
    override fun close(handle: ULong) = Unit
    override fun writable(tree: String) = false
    override fun create(tree: String, parent: String, name: String, directory: Boolean) = HostCreated(2u, "", "")
    override fun openWrite(tree: String, document: String) = HostOpened(2u, 0uL, info())
    override fun write(handle: ULong, data: ByteArray) = 2u
    override fun finishWrite(handle: ULong) = 2u
    override fun rename(tree: String, document: String, name: String) = HostCreated(2u, "", "")
    override fun moveDocument(tree: String, document: String, sourceParent: String, targetParent: String) = HostCreated(2u, "", "")
    override fun delete(tree: String, document: String) = 2u
    override fun resolve(tree: String, path: String) = HostCreated(2u, "", "")
}
