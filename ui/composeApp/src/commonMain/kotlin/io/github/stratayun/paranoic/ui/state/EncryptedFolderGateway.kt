package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

enum class FolderFormat { Paranoic, Cryptomator }

/** One presentation adapter; all format, file and queue logic remains in Rust. */
class EncryptedFolderGateway(private val core: CoreGateway, val format: FolderFormat) {
    private val readOnlyHandles = mutableSetOf<String>()
    /** A session can be restricted, never upgraded in place. Lock and reopen to change mode. */
    fun restrictWrites(handle: String) { readOnlyHandles += handle }
    private suspend fun <T> write(handle: String, action: suspend () -> CoreResult<T>): CoreResult<T> =
        if (handle in readOnlyHandles) CoreResult.Failure(CoreFault.Permission) else action()
    private val compatible get() = format == FolderFormat.Cryptomator
    suspend fun create(parent: Location, name: String, password: String) =
        if (compatible) core.cryptomatorCreate(parent, name, password) else core.vaultCreate(parent, name, password)
    suspend fun open(root: Location, password: String) = if (compatible) core.cryptomatorOpen(root, password) else core.vaultOpen(root, password)
    suspend fun changePassword(handle: String, old: String, new: String) =
        write(handle) { if (compatible) core.cryptomatorChangePassword(handle, old, new) else core.vaultChangePassword(handle, old, new) }
    suspend fun setRecovery(handle: String, password: String): CoreResult<RecoveryDraft> =
        write(handle) { if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultRecoveryPrepare(handle, password) }
    suspend fun openRecovery(root: Location, recovery: String): CoreResult<String> =
        if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultOpenRecovery(root, recovery)
    suspend fun resetPassword(handle: String, recovery: String, newPassword: String): CoreResult<Unit> =
        write(handle) { if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultResetPassword(handle, recovery, newPassword) }
    suspend fun move(handle: String, path: List<String>, entry: VaultEntry, target: List<String>): CoreResult<Unit> =
        write(handle) { if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultMove(handle, path, entry.name, entry.folder, target) }
    suspend fun replace(handle: String, path: List<String>, name: String, source: Location): CoreResult<Unit> =
        write(handle) { if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultReplace(handle, path, name, source) }
    suspend fun list(handle: String, path: List<String>): CoreResult<VaultListing> = if (compatible) {
        when (val result = core.cryptomatorList(handle, path)) {
            is CoreResult.Value -> CoreResult.Value(VaultListing(result.value.entries.map { VaultEntry(it.name, it.folder, it.size, 0) }, result.value.ignored))
            is CoreResult.Failure -> result
        }
    } else core.vaultList(handle, path)
    suspend fun add(handle: String, path: List<String>, source: Location) = write(handle) {
        if (compatible) core.cryptomatorAdd(handle, path, source) else core.vaultAdd(handle, path, source)
    }
    suspend fun addIncoming(handle: String, path: List<String>, name: String, document: IncomingDocument): CoreResult<Unit> = write(handle) {
        if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultAddIncoming(handle, path, name, document)
    }
    suspend fun moveIn(handle: String, path: List<String>, source: Location, requestId: String): CoreResult<VaultMoveInReport> = write(handle) {
        if (compatible) CoreResult.Failure(CoreFault.Unsupported) else core.vaultMoveIn(handle, path, source, requestId)
    }
    suspend fun openStream(handle: String, path: List<String>, name: String): CoreResult<MediaSource> =
        if (compatible) core.cryptomatorOpenStream(handle, path, name) else core.vaultOpenStream(handle, path, name)
    suspend fun pdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int): CoreResult<PdfPage> =
        if (compatible) core.cryptomatorPdfPage(handle, path, name, page, width) else core.vaultPdfPage(handle, path, name, page, width)
    suspend fun pdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int): CoreResult<PdfMatches> =
        if (compatible) core.cryptomatorPdfSearch(handle, path, name, query, start) else core.vaultPdfSearch(handle, path, name, query, start)
    suspend fun officeOpen(handle: String, path: List<String>, name: String): CoreResult<OfficeDocument> =
        if (compatible) core.cryptomatorOfficeOpen(handle, path, name) else core.vaultOfficeOpen(handle, path, name)
    suspend fun textOpen(handle: String, path: List<String>, name: String): CoreResult<VaultTextDocument> =
        if (compatible) core.cryptomatorTextOpen(handle, path, name) else core.vaultTextOpen(handle, path, name)
    suspend fun textSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String): CoreResult<Unit> = write(handle) {
        if (compatible) core.cryptomatorTextSave(handle, path, name, version, text) else core.vaultTextSave(handle, path, name, version, text)
    }
    suspend fun textCreate(handle: String, path: List<String>, name: String, text: String): CoreResult<Unit> = write(handle) {
        if (compatible) core.cryptomatorTextCreate(handle, path, name, text) else core.vaultTextCreate(handle, path, name, text)
    }
    suspend fun archiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?): CoreResult<List<ArchiveEntry>> =
        if (compatible) core.cryptomatorArchiveList(handle, path, name, format, password) else core.vaultArchiveList(handle, path, name, format, password)
    suspend fun archiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?,
        targetPath: List<String>, targetName: String): CoreResult<ULong> = write(handle) {
        if (compatible) core.cryptomatorArchiveExtract(handle, path, name, format, password, targetPath, targetName)
        else core.vaultArchiveExtract(handle, path, name, format, password, targetPath, targetName)
    }
    suspend fun export(handle: String, path: List<String>, name: String, parent: Location) =
        if (compatible) core.cryptomatorExport(handle, path, name, parent) else core.vaultExport(handle, path, name, parent)
    suspend fun mkdir(handle: String, path: List<String>, name: String) = write(handle) {
        if (compatible) core.cryptomatorMkdir(handle, path, name) else core.vaultMkdir(handle, path, name)
    }
    suspend fun rename(handle: String, path: List<String>, name: String, newName: String, folder: Boolean) =
        write(handle) { if (compatible) core.cryptomatorRename(handle, path, name, newName) else core.vaultRename(handle, path, name, newName, folder) }
    suspend fun delete(handle: String, path: List<String>, name: String, folder: Boolean) =
        write(handle) { if (compatible) core.cryptomatorDelete(handle, path, name) else core.vaultDelete(handle, path, name, folder) }
    suspend fun lock(handle: String): CoreResult<Unit> {
        val result = if (compatible) core.cryptomatorLock(handle) else core.vaultLock(handle)
        if (result is CoreResult.Value) readOnlyHandles.remove(handle)
        return result
    }
}
