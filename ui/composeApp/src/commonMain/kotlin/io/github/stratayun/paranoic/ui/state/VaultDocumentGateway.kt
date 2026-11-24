package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

/** Binds existing read-only viewers to one vault document. The synthetic UI
 * identity never reaches Rust as a filesystem Location. No grants or writes. */
internal class VaultDocumentGateway(private val core: CoreGateway, private val gateway: EncryptedFolderGateway,
    private val handle: String, path: List<String>, entry: VaultEntry) : CoreGateway by DisconnectedCoreGateway {
    private val path = path.toList()
    override val connected get() = core.connected
    val file = FileItem(Location("protected-preview", entry.name), entry.name, EntryKind.File, entry.size)
    override suspend fun inspect(location: Location): CoreResult<PreviewInfo> {
        if (location != file.location) return CoreResult.Failure(CoreFault.Permission)
        val (kind, mime) = when (file.name.substringAfterLast('.', "").lowercase()) {
            "pdf" -> PreviewKind.Pdf to "application/pdf"
            "mp3" -> PreviewKind.Audio to "audio/mpeg"
            "wav" -> PreviewKind.Audio to "audio/wav"
            "flac" -> PreviewKind.Audio to "audio/flac"
            "ogg", "opus" -> PreviewKind.Audio to "audio/ogg"
            "m4a" -> PreviewKind.Audio to "audio/mp4"
            "aac" -> PreviewKind.Audio to "audio/aac"
            "mp4" -> PreviewKind.Video to "video/mp4"
            "mov" -> PreviewKind.Video to "video/quicktime"
            "mkv" -> PreviewKind.Video to "video/x-matroska"
            "webm" -> PreviewKind.Video to "video/webm"
            else -> return CoreResult.Failure(CoreFault.Unsupported)
        }
        // MIME selects a renderer only. Rust/platform decoders validate content.
        // Cryptomator's listing size is ciphertext metadata; openMedia supplies
        // the stream's actual plaintext length to the player.
        return CoreResult.Value(PreviewInfo(kind, mime, file.bytes))
    }
    override suspend fun openMedia(location: Location): CoreResult<MediaSource> =
        if (location != file.location) CoreResult.Failure(CoreFault.Permission) else gateway.openStream(handle, path, file.name)
    override suspend fun pdfPage(location: Location, page: Int, width: Int): CoreResult<PdfPage> =
        if (location != file.location) CoreResult.Failure(CoreFault.Permission) else gateway.pdfPage(handle, path, file.name, page, width)
    override suspend fun pdfSearch(location: Location, query: String, start: Int): CoreResult<PdfMatches> =
        if (location != file.location) CoreResult.Failure(CoreFault.Permission) else gateway.pdfSearch(handle, path, file.name, query, start)
    override suspend fun officeOpen(location: Location): CoreResult<OfficeDocument> =
        if (location != file.location) CoreResult.Failure(CoreFault.Permission) else gateway.officeOpen(handle, path, file.name)
    override fun releasePdf() = core.releasePdf()
}
