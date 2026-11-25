package io.github.stratayun.paranoic.platform

import android.content.ContentProvider
import android.content.ContentResolver
import android.content.ContentValues
import android.content.pm.ProviderInfo
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.nativecore.*
import kotlinx.coroutines.runBlocking
import java.io.File
import java.io.FileNotFoundException
import java.util.UUID
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.test.*

/** Fixture resolver only: no picker, persisted grant or cloud account.
 * ContentResolver.wrap requires API 29; real SAF grants need the device checklist.
 */
@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = 29)
class AndroidDocumentsHostTest {
    private lateinit var directory: File
    private lateinit var provider: Provider
    private lateinit var host: AndroidDocumentsHost
    private val tree = DocumentsContract.buildTreeDocumentUri("paranoic.fixture.invalid", "root").toString()
    private data class Node(val id: String, var name: String, val data: ByteArray, val pipe: Boolean = false,
        var parent: String = "root", val directory: Boolean = false, val reportedSize: Long? = data.size.toLong())

    private class Provider(private val directory: File) : ContentProvider() {
        val nodes = linkedMapOf<String, Node>()
        var deny = false
        var documentCalls = 0
        override fun onCreate() = true
        override fun query(uri: Uri, projection: Array<out String>?, selection: String?,
            selectionArgs: Array<out String>?, sortOrder: String?): Cursor {
            if (deny) throw SecurityException("fixture permission revoked")
            val columns = requireNotNull(projection)
            val children = uri.lastPathSegment == "children"
            val id = DocumentsContract.getDocumentId(uri)
            val selected = if (children) nodes.values.filter { it.parent == id } else if (id == "root") emptyList() else
                listOf(nodes[id] ?: throw FileNotFoundException("fixture document missing"))
            return MatrixCursor(columns).apply {
                fun row(node: Node?, directoryRow: Boolean = false): Array<Any?> = columns.map { column ->
                    when (column) {
                        DocumentsContract.Document.COLUMN_DOCUMENT_ID -> node?.id ?: "root"
                        DocumentsContract.Document.COLUMN_DISPLAY_NAME -> node?.name ?: "root"
                        DocumentsContract.Document.COLUMN_MIME_TYPE -> if (directoryRow) DocumentsContract.Document.MIME_TYPE_DIR else "application/octet-stream"
                        DocumentsContract.Document.COLUMN_SIZE -> node?.reportedSize
                        DocumentsContract.Document.COLUMN_LAST_MODIFIED -> 1000L
                        else -> null
                    }
                }.toTypedArray()
                if (!children && id == "root") addRow(row(null, true))
                selected.forEach { addRow(row(it, it.directory)) }
            }
        }
        override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
            documentCalls++
            if (deny) throw SecurityException("fixture permission revoked")
            check(mode == "r")
            val node = nodes[DocumentsContract.getDocumentId(uri)] ?: throw FileNotFoundException()
            if (node.pipe) {
                val ends = ParcelFileDescriptor.createPipe()
                try { ParcelFileDescriptor.AutoCloseOutputStream(ends[1]).use { it.write(node.data) } }
                catch (error: Exception) { ends[0].close(); throw error }
                return ends[0]
            }
            val file = File(directory, node.id).apply { writeBytes(node.data) }
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
        }
        override fun getType(uri: Uri) = "application/octet-stream"
        override fun call(method: String, arg: String?, extras: Bundle?): Bundle? {
            documentCalls++
            // An ancestor may report any descendant as its child. This is
            // deliberately insufficient evidence of an immediate-parent link.
            if (method == "android:isChildDocument") return Bundle().apply { putBoolean("result", true) }
            throw UnsupportedOperationException("fixture document call")
        }
        override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
        override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
        override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
    }

    @Before fun setUp() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        directory = File(context.cacheDir, "test-documents-" + UUID.randomUUID())
        check(directory.mkdir())
        provider = Provider(directory).apply {
            attachInfo(context, ProviderInfo().apply {
                authority = "paranoic.fixture.invalid"; exported = false
                applicationInfo = context.applicationInfo
            })
        }
        host = AndroidDocumentsHost(ContentResolver.wrap(provider))
    }
    @After fun tearDown() { if (::directory.isInitialized) directory.deleteRecursively() }

    @Test fun rawInvalidAndDuplicateNamesReachRustWithoutHidingWholeFolder() {
        provider.nodes["one"] = Node("one", "bad:name", byteArrayOf(1))
        provider.nodes["two"] = Node("two", "same", byteArrayOf(2))
        provider.nodes["three"] = Node("three", "same", byteArrayOf(3))
        val listing = host.list(tree, "", 10u)
        assertEquals(0u, listing.error)
        assertEquals(listOf("bad:name", "same", "same"), listing.entries.map { it.name })
        // Rust SafStorage tests separately check omitted-count and strict-operation rejection.
    }

    @Test fun revokedOrOverLimitListingIsNotSuccessfulEmptyFolder() {
        provider.nodes["one"] = Node("one", "one", byteArrayOf(1))
        provider.nodes["two"] = Node("two", "two", byteArrayOf(2))
        assertEquals(4u, host.list(tree, "", 1u).error)
        provider.deny = true
        assertEquals(1u, host.list(tree, "", 10u).error)
    }

    @Test fun unknownProviderSizeIsRetainedAndRegularDescriptorSuppliesActualSize() {
        provider.nodes["unknown"] = Node("unknown", "unknown.txt", byteArrayOf(1, 2, 3), reportedSize = null)
        val metadata = host.metadata(tree, "unknown.txt")
        assertEquals(0u, metadata.error)
        assertEquals(ULong.MAX_VALUE, metadata.bytes)
        val listing = host.list(tree, "", 10u)
        assertEquals(0u, listing.error)
        assertEquals(listOf("unknown.txt"), listing.entries.map { it.name })
        val opened = host.open(tree, "unknown.txt")
        assertEquals(0u, opened.error)
        try { assertEquals(3uL, host.fileMetadata(opened.id).bytes) }
        finally { host.close(opened.id) }
    }

    @Test fun unsupportedProviderMoveDoesNotFallBackToCopyOrDelete() {
        provider.nodes["one"] = Node("one", "file", byteArrayOf(1))
        val source = host.resolve(tree, "file").document
        val parent = DocumentsContract.buildDocumentUriUsingTree(Uri.parse(tree), "root").toString()
        val calls = provider.documentCalls
        assertEquals(2u, host.moveDocument(tree, source, parent, parent).error)
        assertEquals(calls, provider.documentCalls)
        assertEquals("root", provider.nodes.getValue("one").parent)
    }

    @Test fun renamedCachedDocumentIsResolvedAgainByCurrentName() {
        val original = Node("one", "file", "old".toByteArray())
        provider.nodes["one"] = original
        assertEquals(0u, host.metadata(tree, "file").error)
        original.name = "renamed"
        provider.nodes["two"] = Node("two", "file", "new contents".toByteArray())
        val opened = host.open(tree, "file")
        assertEquals(0u, opened.error)
        try {
            assertContentEquals("new contents".toByteArray(), host.read(opened.id, 0uL, 32u).data)
        } finally { host.close(opened.id) }
    }

    @Test fun cachedDocumentMovedIntoDescendantDoesNotOccupyOldPath() {
        val original = Node("one", "file", "old".toByteArray())
        provider.nodes["one"] = original
        provider.nodes["sub"] = Node("sub", "Sub", byteArrayOf(), directory = true)
        assertEquals(0u, host.metadata(tree, "file").error)
        original.parent = "sub"
        provider.nodes["replacement"] = Node("replacement", "file", "replacement".toByteArray())
        val opened = host.open(tree, "file")
        assertEquals(0u, opened.error)
        try { assertContentEquals("replacement".toByteArray(), host.read(opened.id, 0uL, 32u).data) }
        finally { host.close(opened.id) }
    }

    @Test fun duplicateAddedAfterResolutionCannotUseCachedIdentity() {
        provider.nodes["one"] = Node("one", "file", byteArrayOf(1))
        assertEquals(0u, host.metadata(tree, "file").error)
        provider.nodes["two"] = Node("two", "file", byteArrayOf(2))
        assertNotEquals(0u, host.metadata(tree, "file").error)
    }

    @Test fun publicResolutionReturnsVerifiedIdentityAndRejectsOtherTrees() {
        provider.nodes["one"] = Node("one", "file", byteArrayOf(1, 2, 3))
        val resolved = host.resolve(tree, "file")
        assertEquals(0u, resolved.error)
        assertEquals("file", resolved.name)
        assertEquals("one", DocumentsContract.getDocumentId(Uri.parse(resolved.document)))
        assertEquals(0u, host.metadata(tree, "file").error)
        val callsBefore = provider.documentCalls
        // A host mutation must reject the foreign URI before contacting its provider.
        val foreign = DocumentsContract.buildDocumentUri("other.fixture.invalid", "one").toString()
        assertNotEquals(0u, host.openWrite(tree, foreign).error)
        assertNotEquals(0u, host.rename(tree, foreign, "replacement").error)
        assertNotEquals(0u, host.delete(tree, foreign))
        assertEquals(callsBefore, provider.documentCalls)
        assertEquals("file", provider.nodes.getValue("one").name)
    }

    @Test fun pipeProviderSupportsSequentialReadsButRejectsSeekAndClosedHandle() {
        provider.nodes["pipe"] = Node("pipe", "stream", "pipe".toByteArray(), pipe = true)
        val opened = host.open(tree, "stream")
        assertEquals(0u, opened.error)
        try {
            assertContentEquals("pi".toByteArray(), host.read(opened.id, 0uL, 2u).data)
            assertContentEquals("pe".toByteArray(), host.read(opened.id, 2uL, 2u).data)
            assertEquals(2u, host.read(opened.id, 0uL, 1u).error)
            assertNotEquals(0u, host.read(opened.id, 4uL, 1048577u).error)
        } finally { host.close(opened.id); host.close(opened.id) }
        assertNotEquals(0u, host.read(opened.id, 0uL, 1u).error)
    }

    /** У-61 (phone: "Формат, движок или возможность хранилища недоступны" for
     * every ZIP and 7z). Real packaged bridge and document worker; archives
     * chosen in a SAF folder, from a regular descriptor and from a pipe (the
     * bridge then reads through a private copy). A failure prints the recent
     * Core errors, which name the failing layer. */
    @Test fun archivesFromSafFolderListAndExtractThroughPackagedBridge() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.noBackupFilesDir, "test-archives-" + UUID.randomUUID()).apply { check(mkdir()) }
        val grant = NativeGrant(UUID.randomUUID().toString(), tree, true)
        val store = object : NativeGrantStore {
            private var saved = listOf(grant)
            override fun read() = saved
            override fun write(grants: List<NativeGrant>) { saved = grants }
        }
        val sealer = object : RecordSealer {
            override fun established() = false
            override fun available() = false
            override fun seal(data: ByteArray) = HostBytes(2u, byteArrayOf())
            override fun open(data: ByteArray) = HostBytes(2u, byteArrayOf())
        }
        var gateway: NativeCoreGateway? = null
        try {
            val workspace = File(root, "files")
            val libraries = context.applicationInfo.nativeLibraryDir
            val core = NativeCoreGateway.open(File(root, "state").absolutePath, workspace.absolutePath,
                File(libraries, "libpdfium.so").absolutePath, File(libraries, "libparanoic_pdf_worker.so").absolutePath,
                host, store, sealer, documentWorkers = AndroidDocumentWorkers(context))
            gateway = core
            suspend fun <T> CoreResult<T>.value(step: String): T = (this as? CoreResult.Value<T>)?.value
                ?: error("$step failed: $this; recent Core errors: ${core.diagnostics()}")
            val spaces = core.spaces().value("spaces")
            val app = spaces.first { it.id != grant.id }
            val saf = spaces.first { it.id == grant.id }
            File(workspace, "src").mkdirs()
            File(workspace, "src/a.txt").writeText("архив из SAF")
            File(workspace, "src/b.txt").writeText("second file")
            val expected = listOf("src", "src/a.txt", "src/b.txt")
            for ((format, name) in listOf(ArchiveFormat.Zip to "pack.zip", ArchiveFormat.SevenZip to "pack.7z")) {
                core.archiveCreateWithRequest(listOf(Location(app.id, "src")), format, null, app.root, name,
                    UUID.randomUUID().toString()).value("create $name")
                val bytes = File(workspace, name).readBytes()
                assertTrue(bytes.size < 60_000, "fixture pipe writes everything before returning")
                provider.nodes["file-$name"] = Node("file-$name", name, bytes)
                provider.nodes["pipe-$name"] = Node("pipe-$name", "stream-$name", bytes, pipe = true)
                for (chosen in listOf(name, "stream-$name")) {
                    val listed = core.archiveListWithRequest(Location(saf.id, chosen), format, null, UUID.randomUUID().toString())
                        .value("list $chosen").map { it.path }.sorted()
                    assertEquals(expected, listed, chosen)
                    val out = "out-$chosen"
                    core.archiveExtractWithRequest(Location(saf.id, chosen), format, null, app.root, out,
                        UUID.randomUUID().toString()).value("extract $chosen")
                    assertEquals("архив из SAF", File(workspace, "$out/src/a.txt").readText(), chosen)
                    assertEquals("second file", File(workspace, "$out/src/b.txt").readText(), chosen)
                }
            }
            assertTrue(workspace.list()!!.none { it.startsWith(".pnc-spool") }, "a private archive copy was left behind")
        } finally {
            gateway?.shutdown()
            root.deleteRecursively()
        }
    }
}
