package io.github.stratayun.paranoic.platform

import android.content.ContentResolver
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import io.github.stratayun.paranoic.nativecore.*
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** SAF identity traversal and descriptor IO only. Rust owns every policy
 * decision (create-new protocol, conflicts, deletion rules).
 * Status codes: 0 ok, 1 permission, 2 unsupported, 3 IO, 4 limit, 5 not found.
 */
class AndroidDocumentsHost(private val resolver: ContentResolver,
    localRoots: () -> Map<String, String> = { emptyMap() }) : AndroidDocuments {
    private val deletion = AndroidSafDeletion(localRoots)
    private class Opened(
        val descriptor: ParcelFileDescriptor,
        val input: FileInputStream?,
        val output: FileOutputStream?,
        /** Regular files can seek; pipe-backed provider streams are sequential only. */
        val seekable: Boolean,
        val info: HostInfo,
        var offset: Long = 0,
    )
    private val handles = ConcurrentHashMap<ULong, Opened>()
    private val next = AtomicLong(1)
    /** (tree, path) -> document. Verified by display name on every use. */
    // Audit A13: no URI cache. Every path is resolved again from the provider (resolveUri),
    // so a map of names/URIs only grew without ever being read.
    private val projection = arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.COLUMN_SIZE,
        DocumentsContract.Document.COLUMN_LAST_MODIFIED,
    )
    private class ListingLimit : Exception()
    private companion object {
        const val STALL_MILLIS = 30_000L
        val deadlines: java.util.concurrent.ScheduledExecutorService = java.util.concurrent.Executors.newSingleThreadScheduledExecutor { task ->
            Thread(task, "paranoic-saf-deadline").apply { isDaemon = true }
        }
    }
    private fun error(e: Exception): UInt = when (e) {
        is SecurityException -> 1u
        is UnsupportedOperationException -> 2u
        is ListingLimit -> 4u
        is FileNotFoundException -> 5u
        is ErrnoException -> when (e.errno) {
            OsConstants.EACCES, OsConstants.EPERM -> 1u
            OsConstants.ENOENT -> 5u
            OsConstants.ENOTEMPTY, OsConstants.EISDIR, OsConstants.ENOTDIR, OsConstants.ELOOP -> 2u
            else -> 3u
        }
        else -> 3u
    }
    private fun unavailable(code: UInt) = HostInfo(code, 0u, 0uL, null)
    private fun info(cursor: android.database.Cursor): HostInfo {
        val directory = cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR
        val size = if (cursor.isNull(3)) -1L else cursor.getLong(3)
        val modified = if (cursor.isNull(4)) null else cursor.getLong(4).takeIf { it >= 0 }?.let { (it / 1000 * 1000).toULong() }
        // Agreed bridge sentinel: unknown is neither an empty file nor a failed listing.
        val bytes = if (!directory && size < 0) ULong.MAX_VALUE else size.coerceAtLeast(0).toULong()
        return HostInfo(0u, if (directory) 2u else 1u, bytes, modified)
    }
    private fun root(tree: Uri) = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
    private fun queryInfo(uri: Uri): HostInfo =
        resolver.query(uri, projection, null, null, null)?.use { if (it.moveToFirst()) info(it) else null }
            ?: throw FileNotFoundException("document unavailable")
    private fun displayName(uri: Uri): String? = try {
        resolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null }
    } catch (_: Exception) { null }
    private fun children(tree: Uri, parent: Uri, limit: Int): List<Pair<String, Pair<Uri, HostInfo>>> {
        require(limit in 1..100000)
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getDocumentId(parent))
        return resolver.query(uri, projection, null, null, null)?.use { cursor ->
            val rows = ArrayList<Pair<String, Pair<Uri, HostInfo>>>()
            while (cursor.moveToNext()) {
                if (rows.size >= limit) throw ListingLimit()
                val document = DocumentsContract.buildDocumentUriUsingTree(tree, cursor.getString(0))
                rows.add(cursor.getString(1) to (document to info(cursor)))
            }
            rows
        } ?: throw java.io.IOException("provider query unavailable")
    }
    private fun validPart(part: String) = part.isNotEmpty() && part != "." && part != ".." && part.length <= 255 && '/' !in part
    /** Resolve every step from the immediate parent's current listing.
     * isChildDocument accepts descendants, so it cannot prove that a cached
     * document still occupies this exact path, nor detect a duplicate name. */
    private fun resolveUri(treeText: String, path: String): Uri {
        val tree = Uri.parse(treeText)
        var current = root(tree)
        if (path.isEmpty()) return current
        var prefix = ""
        for (part in path.split('/')) {
            require(validPart(part))
            prefix = if (prefix.isEmpty()) part else "$prefix/$part"
            val matches = children(tree, current, 100000).filter { it.first == part }
            current = when (matches.size) {
                0 -> throw FileNotFoundException("document unavailable")
                1 -> matches[0].second.first
                else -> throw java.io.IOException("ambiguous document name")
            }
        }
        return current
    }
    private fun child(parent: String, name: String) = if (parent.isEmpty()) name else "$parent/$name"

    override fun metadata(tree: String, path: String): HostInfo = try { queryInfo(resolveUri(tree, path)) }
        catch (e: Exception) { unavailable(error(e)) }
    override fun list(tree: String, path: String, limit: UInt): HostListing = try {
        val treeUri = Uri.parse(tree)
        val rows = children(treeUri, resolveUri(tree, path), limit.toInt())
        HostListing(0u, rows.map { HostEntry(it.first, it.second.second) })
    } catch (e: Exception) { HostListing(error(e), emptyList()) }

    private fun descriptorInfo(file: Opened): HostInfo {
        if (!file.seekable) return file.info
        val stat = Os.fstat(file.descriptor.fileDescriptor)
        if (!OsConstants.S_ISREG(stat.st_mode) || stat.st_size < 0) return unavailable(2u)
        return HostInfo(0u, 1u, stat.st_size.toULong(), stat.st_mtime.takeIf { it >= 0 }?.toULong()?.times(1000uL))
    }
    override fun open(tree: String, path: String): HostOpened {
        var descriptor: ParcelFileDescriptor? = null
        return try {
            val uri = resolveUri(tree, path)
            val queried = queryInfo(uri)
            if (queried.error != 0u || queried.kind != 1.toUByte()) throw UnsupportedOperationException("not a file")
            descriptor = resolver.openFileDescriptor(uri, "r") ?: throw java.io.IOException("open failed")
            val stat = Os.fstat(descriptor.fileDescriptor)
            val regular = OsConstants.S_ISREG(stat.st_mode)
            val file = Opened(descriptor, FileInputStream(descriptor.fileDescriptor), null, regular, queried)
            val metadata = descriptorInfo(file)
            if (metadata.error != 0u) throw UnsupportedOperationException("descriptor is not readable")
            val id = next.getAndIncrement().toULong()
            handles[id] = file
            HostOpened(0u, id, metadata)
        } catch (e: Exception) {
            try { descriptor?.close() } catch (_: Exception) {}
            HostOpened(error(e), 0uL, unavailable(error(e)))
        }
    }
    override fun fileMetadata(handle: ULong): HostInfo = try {
        val file = handles[handle] ?: throw java.io.IOException("closed handle")
        synchronized(file) { descriptorInfo(file) }
    } catch (e: Exception) { unavailable(error(e)) }
    override fun read(handle: ULong, offset: ULong, size: UInt): HostBytes = try {
        require(size <= 1048576u && offset <= Long.MAX_VALUE.toULong())
        val file = handles[handle] ?: throw java.io.IOException("closed handle")
        val input = file.input ?: throw UnsupportedOperationException("write handle")
        synchronized(file) {
            if (file.offset != offset.toLong()) {
                if (!file.seekable) throw UnsupportedOperationException("sequential stream")
                input.channel.position(offset.toLong())
            }
            if (!file.seekable) awaitReadable(file.descriptor)
            val output = ByteArray(size.toInt())
            val count = input.read(output).coerceAtLeast(0)
            file.offset = offset.toLong() + count
            HostBytes(0u, output.copyOf(count)).also { output.fill(0) }
        }
    } catch (e: Exception) { HostBytes(error(e), byteArrayOf()) }
    /** Pipe-backed provider streams (cloud/remote providers) can stall forever,
     * and a blocked read cannot be interrupted from another thread. Wait for
     * data with a bound instead (У-15): no data for [STALL_MILLIS] is an IO
     * error, so the operation fails instead of hanging. One read() after a
     * ready poll returns what is available and does not block again. */
    private fun awaitReadable(descriptor: ParcelFileDescriptor) {
        val deadline = System.nanoTime() + STALL_MILLIS * 1_000_000
        while (true) {
            val remaining = (deadline - System.nanoTime()) / 1_000_000
            if (remaining <= 0) throw java.io.IOException("document provider stalled")
            val poll = StructPollfd().apply { fd = descriptor.fileDescriptor; events = OsConstants.POLLIN.toShort() }
            val ready = try { Os.poll(arrayOf(poll), remaining.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()) }
                catch (e: ErrnoException) { if (e.errno == OsConstants.EINTR) continue else throw java.io.IOException(e) }
            if (ready > 0) return // data, end of stream or error: read() reports which
        }
    }
    override fun close(handle: ULong) {
        val file = handles.remove(handle) ?: return
        synchronized(file) {
            try { file.input?.close() } catch (_: Exception) {}
            try { file.output?.close() } catch (_: Exception) {}
            try { file.descriptor.close() } catch (_: Exception) {}
        }
    }

    override fun writable(tree: String): Boolean = try {
        val uri = Uri.parse(tree)
        resolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission }
    } catch (_: Exception) { false }
    override fun create(tree: String, parent: String, name: String, directory: Boolean): HostCreated = try {
        require(validPart(name))
        val mime = if (directory) DocumentsContract.Document.MIME_TYPE_DIR else "application/octet-stream"
        val uri = DocumentsContract.createDocument(resolver, resolveUri(tree, parent), mime, name)
            ?: throw java.io.IOException("create failed")
        val actual = displayName(uri) ?: throw java.io.IOException("created document unreadable")
        HostCreated(0u, actual, uri.toString())
    } catch (e: Exception) { HostCreated(error(e), "", "") }
    /** `document` is the URI returned by create/resolve/rename: no path lookup. */
    private fun document(tree: String, document: String): Uri {
        val uri = Uri.parse(document)
        // Must belong to this grant's tree; never act on a foreign URI.
        require(uri.toString().startsWith("$tree/document/"))
        return uri
    }
    override fun resolve(tree: String, path: String): HostCreated = try {
        val uri = resolveUri(tree, path)
        HostCreated(0u, displayName(uri) ?: throw java.io.IOException("unreadable"), uri.toString())
    } catch (e: Exception) { HostCreated(error(e), "", "") }
    override fun openWrite(tree: String, document: String): HostOpened {
        var descriptor: ParcelFileDescriptor? = null
        return try {
            // Audit A38: a provider that never answers cannot hold the single Core worker forever.
            val signal = android.os.CancellationSignal()
            val timer = deadlines.schedule({ signal.cancel() }, STALL_MILLIS, java.util.concurrent.TimeUnit.MILLISECONDS)
            descriptor = try { resolver.openFileDescriptor(document(tree, document), "w", signal) } finally { timer.cancel(false) }
                ?: throw java.io.IOException("open failed")
            val file = Opened(descriptor, null, FileOutputStream(descriptor.fileDescriptor), false, HostInfo(0u, 1u, 0uL, null))
            val id = next.getAndIncrement().toULong()
            handles[id] = file
            HostOpened(0u, id, file.info)
        } catch (e: Exception) {
            try { descriptor?.close() } catch (_: Exception) {}
            HostOpened(error(e), 0uL, unavailable(error(e)))
        }
    }
    override fun write(handle: ULong, data: ByteArray): UInt = try {
        val file = handles[handle] ?: throw java.io.IOException("closed handle")
        val output = file.output ?: throw UnsupportedOperationException("read handle")
        try {
            synchronized(file) {
                if (isPipe(file.descriptor)) {
                    // Audit A38: a pipe to a provider may stop draining. Write in PIPE_BUF pieces
                    // after a bounded wait for room, so a stalled provider fails instead of hanging.
                    var at = 0
                    while (at < data.size) {
                        awaitWritable(file.descriptor)
                        val n = minOf(4096, data.size - at)
                        output.write(data, at, n); at += n
                    }
                } else output.write(data)
                file.offset += data.size
            }
        } finally { data.fill(0) }
        0u
    } catch (e: Exception) { data.fill(0); error(e) }
    private fun isPipe(descriptor: ParcelFileDescriptor): Boolean = try {
        val mode = Os.fstat(descriptor.fileDescriptor).st_mode
        OsConstants.S_ISFIFO(mode) || OsConstants.S_ISSOCK(mode)
    } catch (_: Exception) { true }
    private fun awaitWritable(descriptor: ParcelFileDescriptor) {
        val deadline = System.nanoTime() + STALL_MILLIS * 1_000_000
        while (true) {
            val remaining = (deadline - System.nanoTime()) / 1_000_000
            if (remaining <= 0) throw java.io.IOException("document provider stalled")
            val poll = StructPollfd().apply { fd = descriptor.fileDescriptor; events = OsConstants.POLLOUT.toShort() }
            val ready = try { Os.poll(arrayOf(poll), remaining.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()) }
                catch (e: ErrnoException) { if (e.errno == OsConstants.EINTR) continue else throw java.io.IOException(e) }
            if (ready > 0) return
        }
    }
    override fun finishWrite(handle: ULong): UInt {
        val file = handles.remove(handle) ?: return 3u
        return synchronized(file) {
            try {
                val output = file.output ?: throw UnsupportedOperationException("read handle")
                output.flush()
                // Only pipes/sockets (provider-backed writes) cannot fsync. For a
                // regular file a failed sync is a failed write, never success
                // (external audit, 2026-11-14).
                try { file.descriptor.fileDescriptor.sync() } catch (e: java.io.SyncFailedException) {
                    val regular = try {
                        android.system.OsConstants.S_ISREG(android.system.Os.fstat(file.descriptor.fileDescriptor).st_mode)
                    } catch (_: Exception) { true }
                    if (regular) throw e
                }
                output.close()
                file.descriptor.close() // providers commit pipe-backed writes on close
                0u
            } catch (e: Exception) {
                try { file.descriptor.closeWithError("write failed") } catch (_: Exception) {}
                error(e)
            }
        }
    }
    override fun rename(tree: String, document: String, name: String): HostCreated = try {
        require(validPart(name))
        val uri = document(tree, document)
        val renamed = DocumentsContract.renameDocument(resolver, uri, name) ?: uri
        val actual = displayName(renamed) ?: throw java.io.IOException("renamed document unreadable")
        HostCreated(0u, actual, renamed.toString())
    } catch (e: Exception) { HostCreated(error(e), "", "") }
    override fun moveDocument(tree: String, document: String, sourceParent: String, targetParent: String): HostCreated = try {
        if (android.os.Build.VERSION.SDK_INT < 24) throw UnsupportedOperationException("document moves unavailable")
        val treeUri = Uri.parse(tree)
        // Match Rust's trusted-writer boundary. A preflight alone cannot make an
        // arbitrary provider's move operation preserve an existing target.
        if (treeUri.authority != "com.android.externalstorage.documents")
            throw UnsupportedOperationException("provider move guarantees unavailable")
        val source = document(tree, document)
        val from = document(tree, sourceParent)
        val to = document(tree, targetParent)
        if (from == to || source == root(treeUri) || source == to)
            throw UnsupportedOperationException("invalid document move")
        if (queryInfo(from).kind != 2.toUByte() || queryInfo(to).kind != 2.toUByte())
            throw UnsupportedOperationException("move parent is not a directory")
        val name = displayName(source) ?: throw FileNotFoundException("document unavailable")
        require(validPart(name))
        val siblings = children(treeUri, from, 100000)
        val matching = siblings.filter { it.first == name }
        if (matching.size != 1 || matching.single().second.first != source)
            throw java.io.IOException("source parent changed")
        if (children(treeUri, to, 100000).any { it.first == name })
            throw UnsupportedOperationException("target already exists")
        // No copy/delete fallback and no automatic retry: a failed reply can
        // follow a completed move. Rust owns reconciliation and rename/rollback.
        val moved = DocumentsContract.moveDocument(resolver, source, from, to)
            ?: throw java.io.IOException("move result unavailable")
        val verified = document(tree, moved.toString())
        val actual = displayName(verified) ?: throw java.io.IOException("moved document unreadable")
        HostCreated(0u, actual, verified.toString())
    } catch (e: Exception) { HostCreated(error(e), "", "") }
    override fun deletionSupported(tree: String): Boolean = writable(tree) && deletion.supported(Uri.parse(tree))

    /** Typed kernel operation; never invoke a provider's recursive deleteDocument. */
    override fun delete(tree: String, document: String, directory: Boolean): UInt = try {
        if (!writable(tree)) throw SecurityException("write grant unavailable")
        val uri = document(tree, document)
        deletion.delete(Uri.parse(tree), uri, directory)
        0u
    } catch (e: Exception) { error(e) }
}
