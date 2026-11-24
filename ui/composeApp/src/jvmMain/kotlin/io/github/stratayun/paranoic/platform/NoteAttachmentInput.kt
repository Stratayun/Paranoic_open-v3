package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.*
import java.io.InputStream

/** Actual bytes, not provider metadata, enforce the limit. All temporary buffers are wiped. */
internal suspend fun readNoteAttachment(open: () -> InputStream): CoreResult<ByteArray> {
    var output: ByteArray? = null
    var delivered = false
    try {
        val result = withContext(Dispatchers.IO) {
            val staging = ByteArray(NOTE_ATTACHMENT_MAX_BYTES + 1)
            try {
                var size = 0
                open().use { input ->
                    while (size < staging.size) {
                        currentCoroutineContext().ensureActive()
                        val count = input.read(staging, size, minOf(65536, staging.size - size))
                        if (count < 0) break
                        if (count == 0) { // Non-conforming providers must not cause an endless zero-read loop.
                            val byte = input.read()
                            if (byte < 0) break
                            staging[size++] = byte.toByte()
                        } else size += count
                    }
                }
                if (size > NOTE_ATTACHMENT_MAX_BYTES) CoreResult.Failure(CoreFault.Limit)
                else CoreResult.Value(staging.copyOf(size).also { output = it })
            } finally { staging.fill(0) }
        }
        currentCoroutineContext().ensureActive()
        delivered = result is CoreResult.Value
        return result
    } catch (cancelled: CancellationException) { throw cancelled }
    catch (_: SecurityException) { return CoreResult.Failure(CoreFault.Permission) }
    catch (_: Exception) { return CoreResult.Failure(CoreFault.Unavailable) }
    finally { if (!delivered) output?.fill(0) }
}
