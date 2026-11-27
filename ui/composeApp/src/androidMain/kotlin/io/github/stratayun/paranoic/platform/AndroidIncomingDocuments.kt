package io.github.stratayun.paranoic.platform

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/** Temporary, activity-scoped read access for files shared from other apps. */
class AndroidIncomingDocuments(private val resolver: ContentResolver, private val scope: CoroutineScope) : IncomingDocuments {
    override var documents by mutableStateOf<List<IncomingDocument>>(emptyList())
        private set
    override var fault by mutableStateOf<CoreFault?>(null)
        private set
    private val all = ConcurrentHashMap<String, AndroidIncomingDocument>()
    private val metadata = Mutex()
    private var pending = 0
    private var destroyed = false

    fun accept(intent: Intent) {
        if (destroyed || intent.action !in listOf(Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE)) return
        val uris = try { sharedUris(intent) } catch (_: Exception) { fault = CoreFault.Unsupported; return }
        if (uris.isEmpty() || uris.any { it.scheme != ContentResolver.SCHEME_CONTENT }) { fault = CoreFault.Unsupported; return }
        if (uris.size + pending + all.size > 8) { fault = CoreFault.Limit; return }
        pending += uris.size; fault = null
        scope.launch {
            val prepared = mutableListOf<AndroidIncomingDocument>()
            var adopted = false
            try {
                metadata.withLock { withContext(Dispatchers.IO) {
                    uris.forEach { uri ->
                        ensureActive()
                        val (name, size) = describe(uri)
                        prepared += AndroidIncomingDocument(resolver, uri, name, size) { id -> all.remove(id) }
                    }
                } }
                currentCoroutineContext().ensureActive()
                if (!destroyed) {
                    prepared.forEach { all[it.id] = it }
                    documents = documents + prepared
                    adopted = true
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: SecurityException) { fault = CoreFault.Permission }
            catch (_: UnsupportedOperationException) { fault = CoreFault.Unsupported }
            catch (_: Exception) { fault = CoreFault.Unavailable }
            finally {
                pending -= uris.size
                if (!adopted) prepared.forEach { it.discard() }
            }
        }
    }
    private fun describe(uri: Uri): Pair<String, ULong> {
        var name = "shared-file"
        var size: Long? = null
        try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val n = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    val s = cursor.getColumnIndex(OpenableColumns.SIZE)
                    if (n >= 0 && !cursor.isNull(n)) name = cursor.getString(n).take(1024).ifEmpty { name }
                    if (s >= 0 && !cursor.isNull(s)) size = cursor.getLong(s).takeIf { it >= 0 }
                }
            }
        } catch (denied: SecurityException) { throw denied }
        catch (_: Exception) { /* A readable FD can still report the exact size. */ }
        if (size == null) resolver.openFileDescriptor(uri, "r")?.use { fd -> size = fd.statSize.takeIf { it >= 0 } }
        return name to (size ?: throw UnsupportedOperationException("exact incoming size unavailable")).toULong()
    }
    override fun take(id: String): IncomingDocument? {
        val item = documents.firstOrNull { it.id == id } ?: return null
        documents = documents.filterNot { it.id == id }
        return item
    }
    override fun discard(id: String) {
        documents = documents.filterNot { it.id == id }
        all[id]?.discard()
    }
    override fun clear() {
        documents = emptyList(); fault = null
        all.values.toList().forEach { it.discard() }
    }
    fun destroy() { destroyed = true; clear() }

    @Suppress("DEPRECATION")
    private fun sharedUris(intent: Intent): List<Uri> {
        val result = mutableListOf<Uri>()
        if (intent.action == Intent.ACTION_SEND) {
            val uri = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            if (uri != null) result += uri
        } else {
            val values = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
            if ((values?.size ?: 0) > 8) throw UnsupportedOperationException()
            result += values.orEmpty()
        }
        val clip = intent.clipData
        if (clip != null) {
            if (clip.itemCount > 8) throw UnsupportedOperationException()
            for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let { result += it }
        }
        return result.distinct().also { if (it.size > 8) throw UnsupportedOperationException() }
    }
}

private class AndroidIncomingDocument(private val resolver: ContentResolver, uri: Uri, override val name: String,
    override val size: ULong, private val released: (String) -> Unit) : IncomingDocument {
    override val id = UUID.randomUUID().toString()
    override val modifiedMs: ULong? = null
    private val reference = AtomicReference<Uri?>(uri)
    private val claimed = AtomicBoolean(false)
    private val source = AtomicReference<AndroidIncomingSource?>(null)
    override fun open(): IncomingByteSource {
        if (!claimed.compareAndSet(false, true)) throw IOException("incoming capability already used")
        val uri = reference.get() ?: throw IOException("incoming capability discarded")
        val fd = resolver.openFileDescriptor(uri, "r") ?: throw IOException("incoming document unavailable")
        try {
            val stat = Os.fstat(fd.fileDescriptor)
            if (OsConstants.S_ISDIR(stat.st_mode)) throw UnsupportedOperationException("incoming directory")
            if (fd.statSize >= 0 && fd.statSize.toULong() != size) throw IOException("incoming size changed")
            val opened = AndroidIncomingSource(fd, OsConstants.S_ISREG(stat.st_mode))
            source.set(opened)
            if (reference.get() == null) { opened.cancel(); opened.close(); throw IOException("incoming capability discarded") }
            return opened
        } catch (error: Exception) { try { fd.close() } catch (_: Exception) {}; throw error }
    }
    override fun discard() { reference.set(null); source.get()?.cancel(); released(id) }
}

private class AndroidIncomingSource(private val descriptor: ParcelFileDescriptor, private val regular: Boolean) : IncomingByteSource {
    private val input = ParcelFileDescriptor.AutoCloseInputStream(descriptor)
    private val cancelled = AtomicBoolean(false)
    private var closed = false
    private var previous: ByteArray? = null
    override fun cancel() { cancelled.set(true) }
    @Synchronized override fun read(max: Int): ByteArray {
        if (cancelled.get() || closed) throw IOException("incoming stream cancelled")
        require(max in 1..1_048_576)
        previous?.fill(0); previous = null
        if (!regular) awaitReadable()
        val buffer = ByteArray(minOf(max, 65_536))
        try {
            val count = input.read(buffer)
            if (count == 0) throw IOException("incoming stream made no progress")
            val result = if (count < 0) byteArrayOf() else buffer.copyOf(count)
            if (cancelled.get()) { result.fill(0); throw IOException("incoming stream cancelled") }
            previous = result
            return result
        } finally { buffer.fill(0) }
    }
    private fun awaitReadable() {
        val deadline = System.nanoTime() + 30_000_000_000L
        while (!cancelled.get()) {
            val left = (deadline - System.nanoTime()) / 1_000_000
            if (left <= 0) throw IOException("incoming provider stalled")
            val poll = StructPollfd().apply { fd = descriptor.fileDescriptor; events = OsConstants.POLLIN.toShort() }
            val ready = try { Os.poll(arrayOf(poll), minOf(left, 50L).toInt().coerceAtLeast(1)) }
                catch (error: ErrnoException) { if (error.errno == OsConstants.EINTR) continue else throw IOException("incoming provider failed") }
            if (ready > 0) return
        }
        throw IOException("incoming stream cancelled")
    }
    @Synchronized override fun close() {
        cancel()
        if (!closed) { closed = true; previous?.fill(0); previous = null; input.close() }
    }
}
