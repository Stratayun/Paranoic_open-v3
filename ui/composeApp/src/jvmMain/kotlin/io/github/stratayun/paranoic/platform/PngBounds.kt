package io.github.stratayun.paranoic.platform

import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

/** Shared preflight for PDF pages and thumbnails, before native pixel allocation. */
internal object PngBounds {
    const val MAX_ENCODED_BYTES = 64 * 1024 * 1024
    const val MAX_DIMENSION = 4096
    const val MAX_PIXEL_BYTES = 64L * 1024 * 1024
    private val signature = byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)

    data class Size(val width: Int, val height: Int)

    fun read(bytes: ByteArray): Size {
        require(bytes.size in 33..MAX_ENCODED_BYTES) { "PNG byte limit" }
        require(signature.indices.all { bytes[it] == signature[it] }) { "Not PNG" }
        require(u32(bytes, 8) == 13L && bytes[12] == 0x49.toByte() && bytes[13] == 0x48.toByte()
            && bytes[14] == 0x44.toByte() && bytes[15] == 0x52.toByte()) { "Invalid PNG header" }
        val width = u32(bytes, 16)
        val height = u32(bytes, 20)
        require(width in 1L..MAX_DIMENSION.toLong() && height in 1L..MAX_DIMENSION.toLong()) { "PNG dimensions" }
        // Long arithmetic: hostile unsigned dimensions must never overflow Int.
        require(width * height <= MAX_PIXEL_BYTES / 4) { "PNG pixel limit" }
        return Size(width.toInt(), height.toInt())
    }

    private fun u32(bytes: ByteArray, offset: Int): Long =
        (0..3).fold(0L) { value, index -> (value shl 8) or (bytes[offset + index].toLong() and 255L) }
}

/** Cancellation while waiting is immediate; at most one native decode runs at once. */
internal object PngDecodeBudget {
    private val permit = Semaphore(1)
    suspend fun <T> decode(block: suspend () -> T): T = permit.withPermit { block() }
}
