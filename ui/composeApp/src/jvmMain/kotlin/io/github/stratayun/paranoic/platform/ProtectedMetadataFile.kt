package io.github.stratayun.paranoic.platform

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.file.*
import java.nio.file.attribute.BasicFileAttributes

internal interface CatalogProtection {
    fun seal(data: ByteArray, createKey: Boolean): ByteArray
    fun open(data: ByteArray): ByteArray
}

/** Host-private metadata only. Atomic replacement has no unprotected fallback. */
internal class ProtectedMetadataFile(private val file: Path, private val protection: CatalogProtection) {
    suspend fun read(): ByteArray? = withContext(Dispatchers.IO) {
        val metadata = try { Files.readAttributes(file, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS) }
            catch (_: NoSuchFileException) { return@withContext null }
        require(metadata.isRegularFile && metadata.size() <= MAX_BYTES)
        val data = FileChannel.open(file, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS).use { channel ->
            val bytes = ByteArrayOutputStream(); val buffer = ByteBuffer.allocate(8192)
            while (true) {
                buffer.clear()
                val count = channel.read(buffer); if (count < 0) break
                require(bytes.size() + count <= MAX_BYTES)
                bytes.write(buffer.array(), 0, count)
            }
            bytes.toByteArray()
        }
        val plain = protection.open(data)
        if (plain.size > MAX_BYTES) { plain.fill(0); error("Metadata too large") }
        plain
    }

    suspend fun write(plain: ByteArray) = withContext(Dispatchers.IO) {
        require(plain.size <= MAX_BYTES)
        Files.createDirectories(file.parent)
        val exists = Files.exists(file, LinkOption.NOFOLLOW_LINKS)
        require(!exists || Files.isRegularFile(file, LinkOption.NOFOLLOW_LINKS))
        val data = protection.seal(plain, !exists)
        require(data.size <= MAX_BYTES)
        val temporary = Files.createTempFile(file.parent, "catalog-", ".tmp")
        try {
            FileChannel.open(temporary, StandardOpenOption.WRITE).use { channel ->
                val bytes = ByteBuffer.wrap(data)
                while (bytes.hasRemaining()) channel.write(bytes)
                channel.force(true)
            }
            Files.move(temporary, file, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            Unit
        } finally { Files.deleteIfExists(temporary) }
    }
    companion object { const val MAX_BYTES = 1024 * 1024 }
}
