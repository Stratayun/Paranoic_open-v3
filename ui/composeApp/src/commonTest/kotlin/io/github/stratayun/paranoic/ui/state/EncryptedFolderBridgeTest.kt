package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.runBlocking
import kotlin.test.*

/** These exercise Kotlin routing and ownership, not Rust crypto or platform IO. */
class EncryptedFolderBridgeTest {
    @Test fun moveKeepsTheConfirmedCopyResultWithoutDeletingOrRetryingTheSource() = runBlocking {
        val expectedSource = Location("device", "source.txt")
        val reply = VaultMoveInReport(123uL, sourceRemoved = false)
        var calls = 0
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override suspend fun vaultMoveIn(handle: String, path: List<String>, source: Location, requestId: String): CoreResult<VaultMoveInReport> {
                assertEquals("safe", handle); assertEquals(listOf("folder"), path)
                assertEquals(expectedSource, source); assertEquals("fixed-request", requestId)
                calls++; return CoreResult.Value(reply)
            }
            override suspend fun delete(source: Location, recursive: Boolean, requestId: String): CoreResult<JobItem> =
                error("Kotlin must never implement a second source deletion")
        }
        val gateway = EncryptedFolderGateway(core, FolderFormat.Paranoic)
        assertEquals(CoreResult.Value(reply), gateway.moveIn("safe", listOf("folder"), expectedSource, "fixed-request"))
        assertEquals(1, calls)
        // A missing Cryptomator move API cannot fall back to a copy/delete pair.
        assertEquals(CoreResult.Failure(CoreFault.Unsupported), EncryptedFolderGateway(core, FolderFormat.Cryptomator)
            .moveIn("safe", emptyList(), expectedSource, "other-request"))
        assertEquals(1, calls)
    }

    @Test fun textRetainsOpaqueVersionAndChangedNeverTurnsIntoAnAutomaticCopy() = runBlocking {
        for (format in FolderFormat.entries) {
            val core = RecordingCore()
            val gateway = EncryptedFolderGateway(core, format)
            val opened = assertIs<CoreResult.Value<VaultTextDocument>>(gateway.textOpen("vault", listOf("a"), "note.txt")).value
            assertSame(core.text, opened)
            assertEquals("opaque-strong-version", opened.version.token)
            assertEquals(CoreResult.Failure(CoreFault.Changed), gateway.textSave("vault", listOf("a"), "note.txt", opened.version, "draft"))
            assertSame(opened.version, core.savedVersion)
            assertEquals("draft", core.savedText)
            assertEquals(listOf(prefix(format) + "text-open", prefix(format) + "text-save"), core.calls.map { it.operation })
            // Creating a copy requires its own explicit invocation.
            assertEquals(CoreResult.Value(Unit), gateway.textCreate("vault", listOf("a"), "copy.txt", "draft"))
            assertEquals(prefix(format) + "text-create", core.calls.last().operation)
        }
    }

    @Test fun archivesKeepFormatPasswordAndEncryptedDestinationForBothVaults() = runBlocking {
        for (format in FolderFormat.entries) {
            val core = RecordingCore()
            val gateway = EncryptedFolderGateway(core, format)
            assertEquals(CoreResult.Value(core.entries), gateway.archiveList("vault", listOf("a"), "input.7z", ArchiveFormat.SevenZip, "secret"))
            assertEquals(listOf(ArchiveFormat.SevenZip, "secret"), core.archiveArguments)
            assertEquals(CoreResult.Value(2uL), gateway.archiveExtract("vault", listOf("a"), "input.7z", ArchiveFormat.SevenZip, "secret", listOf("b"), "new"))
            assertEquals(listOf(prefix(format) + "archive-list", prefix(format) + "archive-extract"), core.calls.map { it.operation })
            assertTrue(core.calls.all { it.handle == "vault" && it.path == listOf("a") && it.name == "input.7z" })
            assertEquals(listOf(ArchiveFormat.SevenZip, "secret", listOf("b"), "new"), core.archiveArguments)
        }
    }

    @Test fun viewerReadsUseTheBoundVaultIdentityAndSnapshotOfItsPath() = runBlocking {
        for (format in FolderFormat.entries) {
            val core = RecordingCore()
            val folders = EncryptedFolderGateway(core, format)
            folders.restrictWrites("vault")
            val path = mutableListOf("original")
            val viewer = VaultDocumentGateway(core, folders, "vault", path, VaultEntry("file.pdf", false, 123uL, 0))
            path[0] = "different"
            val location = viewer.file.location
            assertEquals(PreviewKind.Pdf, assertIs<CoreResult.Value<PreviewInfo>>(viewer.inspect(location)).value.kind)
            assertEquals(CoreResult.Value(core.page), viewer.pdfPage(location, 3, 2048))
            assertEquals(CoreResult.Value(core.matches), viewer.pdfSearch(location, "word", 4))
            assertEquals(CoreResult.Value(core.office), viewer.officeOpen(location))
            assertEquals(CoreResult.Value(core.stream), viewer.openMedia(location))
            viewer.releasePdf()
            assertEquals(1, core.pdfReleases)
            assertEquals(listOf("pdf-page", "pdf-search", "office", "stream").map { prefix(format) + it }, core.calls.map { it.operation })
            assertTrue(core.calls.all { it.handle == "vault" && it.path == listOf("original") && it.name == "file.pdf" })
            assertEquals(listOf(3, 2048), core.pageArguments)
            assertEquals(listOf("word", 4), core.searchArguments)
        }
    }

    @Test fun viewerRefusesOtherIdentitiesAndCannotReachOrdinaryStorageOrMutation() = runBlocking {
        val core = RecordingCore()
        val viewer = VaultDocumentGateway(core, EncryptedFolderGateway(core, FolderFormat.Paranoic), "vault", emptyList(), VaultEntry("file.pdf", false, 0uL, 0))
        val other = Location("disk", "file.pdf")
        val denied = CoreResult.Failure(CoreFault.Permission)
        assertEquals(denied, viewer.inspect(other))
        assertEquals(denied, viewer.pdfPage(other, 0, 2048))
        assertEquals(denied, viewer.pdfSearch(other, "x", 0))
        assertEquals(denied, viewer.officeOpen(other))
        assertEquals(denied, viewer.openMedia(other))
        assertIs<CoreResult.Failure>(viewer.spaces())
        assertIs<CoreResult.Failure>(viewer.list(viewer.file.location))
        assertIs<CoreResult.Failure>(viewer.createDirectory(viewer.file.location, "new", "request"))
        assertIs<CoreResult.Failure>(viewer.vaultTextCreate("vault", emptyList(), "new.txt", "draft"))
        assertTrue(core.calls.isEmpty())
    }

    private fun prefix(format: FolderFormat) = if (format == FolderFormat.Paranoic) "vault-" else "cryptomator-"
}

private data class BridgeCall(val operation: String, val handle: String, val path: List<String>, val name: String)

private class RecordingCore : CoreGateway by DisconnectedCoreGateway {
    override val connected = true
    val calls = mutableListOf<BridgeCall>()
    val text = VaultTextDocument("original", TextEncoding.Utf16Le, VaultTextVersion(16uL, 10uL, "opaque-strong-version"))
    val page = PdfPage(5, 3, byteArrayOf(1, 2))
    val matches = PdfMatches(listOf(4), null)
    val office = OfficeDocument.Workbook(listOf(OfficeSheet("Sheet", listOf(listOf("cell")), false)), false)
    val entries = listOf(ArchiveEntry("a.txt", false, 2uL, 0uL, true))
    val stream = object : MediaSource {
        override val length = 0L
        override fun readAt(offset: Long, size: Int) = byteArrayOf()
        override fun close() = Unit
    }
    var savedVersion: VaultTextVersion? = null
    var savedText: String? = null
    var archiveArguments: List<Any?> = emptyList()
    var pageArguments: List<Int> = emptyList()
    var searchArguments: List<Any> = emptyList()
    var pdfReleases = 0
    private fun call(operation: String, handle: String, path: List<String>, name: String) { calls += BridgeCall(operation, handle, path.toList(), name) }
    private fun textOpen(prefix: String, h: String, p: List<String>, n: String): CoreResult<VaultTextDocument> { call(prefix + "text-open", h, p, n); return CoreResult.Value(text) }
    private fun textSave(prefix: String, h: String, p: List<String>, n: String, v: VaultTextVersion, text: String): CoreResult<Unit> {
        call(prefix + "text-save", h, p, n); savedVersion = v; savedText = text; return CoreResult.Failure(CoreFault.Changed)
    }
    private fun textCreate(prefix: String, h: String, p: List<String>, n: String): CoreResult<Unit> { call(prefix + "text-create", h, p, n); return CoreResult.Value(Unit) }
    private fun archiveList(prefix: String, h: String, p: List<String>, n: String, f: ArchiveFormat, password: String?): CoreResult<List<ArchiveEntry>> {
        call(prefix + "archive-list", h, p, n); archiveArguments = listOf(f, password); return CoreResult.Value(entries)
    }
    private fun archiveExtract(prefix: String, h: String, p: List<String>, n: String, f: ArchiveFormat, password: String?, target: List<String>, name: String): CoreResult<ULong> {
        call(prefix + "archive-extract", h, p, n); archiveArguments = listOf(f, password, target, name); return CoreResult.Value(2uL)
    }
    private fun page(prefix: String, h: String, p: List<String>, n: String, index: Int, width: Int): CoreResult<PdfPage> {
        call(prefix + "pdf-page", h, p, n); pageArguments = listOf(index, width); return CoreResult.Value(page)
    }
    private fun search(prefix: String, h: String, p: List<String>, n: String, query: String, start: Int): CoreResult<PdfMatches> {
        call(prefix + "pdf-search", h, p, n); searchArguments = listOf(query, start); return CoreResult.Value(matches)
    }
    private fun office(prefix: String, h: String, p: List<String>, n: String): CoreResult<OfficeDocument> { call(prefix + "office", h, p, n); return CoreResult.Value(office) }
    private fun stream(prefix: String, h: String, p: List<String>, n: String): CoreResult<MediaSource> { call(prefix + "stream", h, p, n); return CoreResult.Value(stream) }
    override suspend fun vaultTextOpen(handle: String, path: List<String>, name: String) = textOpen("vault-", handle, path, name)
    override suspend fun cryptomatorTextOpen(handle: String, path: List<String>, name: String) = textOpen("cryptomator-", handle, path, name)
    override suspend fun vaultTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String) = textSave("vault-", handle, path, name, version, text)
    override suspend fun cryptomatorTextSave(handle: String, path: List<String>, name: String, version: VaultTextVersion, text: String) = textSave("cryptomator-", handle, path, name, version, text)
    override suspend fun vaultTextCreate(handle: String, path: List<String>, name: String, text: String) = textCreate("vault-", handle, path, name)
    override suspend fun cryptomatorTextCreate(handle: String, path: List<String>, name: String, text: String) = textCreate("cryptomator-", handle, path, name)
    override suspend fun vaultArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?) = archiveList("vault-", handle, path, name, format, password)
    override suspend fun cryptomatorArchiveList(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?) = archiveList("cryptomator-", handle, path, name, format, password)
    override suspend fun vaultArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?, targetPath: List<String>, targetName: String) = archiveExtract("vault-", handle, path, name, format, password, targetPath, targetName)
    override suspend fun cryptomatorArchiveExtract(handle: String, path: List<String>, name: String, format: ArchiveFormat, password: String?, targetPath: List<String>, targetName: String) = archiveExtract("cryptomator-", handle, path, name, format, password, targetPath, targetName)
    override suspend fun vaultPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int) = page("vault-", handle, path, name, page, width)
    override suspend fun cryptomatorPdfPage(handle: String, path: List<String>, name: String, page: Int, width: Int) = page("cryptomator-", handle, path, name, page, width)
    override suspend fun vaultPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int) = search("vault-", handle, path, name, query, start)
    override suspend fun cryptomatorPdfSearch(handle: String, path: List<String>, name: String, query: String, start: Int) = search("cryptomator-", handle, path, name, query, start)
    override suspend fun vaultOfficeOpen(handle: String, path: List<String>, name: String) = office("vault-", handle, path, name)
    override suspend fun cryptomatorOfficeOpen(handle: String, path: List<String>, name: String) = office("cryptomator-", handle, path, name)
    override suspend fun vaultOpenStream(handle: String, path: List<String>, name: String) = stream("vault-", handle, path, name)
    override suspend fun cryptomatorOpenStream(handle: String, path: List<String>, name: String) = stream("cryptomator-", handle, path, name)
    override fun releasePdf() { pdfReleases++ }
}
