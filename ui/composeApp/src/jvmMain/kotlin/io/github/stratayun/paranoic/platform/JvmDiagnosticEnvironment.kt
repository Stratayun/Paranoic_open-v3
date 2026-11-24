package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.*
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.file.*
import java.nio.file.attribute.BasicFileAttributes
import java.util.Collections
import java.util.IdentityHashMap

/** Only code identities are recorded. Throwable messages, paths and thread names are never read. */
internal class JvmDiagnosticEnvironment(private val file: Path, private val platform: () -> DiagnosticPlatformFacts) : DiagnosticEnvironment {
    override suspend fun facts() = withContext(Dispatchers.IO) { platform() }
    override suspend fun previousCrash(): String? = withContext(Dispatchers.IO) {
        val attributes = try { Files.readAttributes(file, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS) }
            catch (_: NoSuchFileException) { return@withContext null }
        require(attributes.isRegularFile && attributes.size() in 1..MAX_CRASH_BYTES.toLong())
        val data = FileChannel.open(file, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS).use { channel ->
            val buffer = ByteBuffer.allocate(MAX_CRASH_BYTES + 1)
            while (channel.read(buffer) >= 0) { require(buffer.hasRemaining()) }
            buffer.array().copyOf(buffer.position())
        }
        decodeCrash(data)
    }
    override suspend fun clearCrash() = withContext(Dispatchers.IO) { Files.deleteIfExists(file); Unit }

    fun install() {
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        if (previous is Recorder) return
        Thread.setDefaultUncaughtExceptionHandler(Recorder(previous))
    }
    private inner class Recorder(private val previous: Thread.UncaughtExceptionHandler?) : Thread.UncaughtExceptionHandler {
        override fun uncaughtException(thread: Thread, error: Throwable) {
            // Fatal-path best effort only. Disk IO cannot indefinitely delay the platform's terminating handler.
            try {
                val writer = Thread({ try { record(error) } catch (_: Throwable) {} }, "paranoic-crash-record").apply { isDaemon = true }
                writer.start(); writer.join(250)
            } catch (_: InterruptedException) { Thread.currentThread().interrupt() }
            catch (_: Throwable) { /* Preserve the platform's fatal handler. */ }
            if (previous != null) previous.uncaughtException(thread, error)
            else System.err.println("Paranoic: uncaught ${codeIdentity(error.javaClass.name)}")
        }
    }
    internal fun record(error: Throwable) {
        val data = encodeCrash(error, System.currentTimeMillis())
        Files.createDirectories(file.parent)
        val temporary = Files.createTempFile(file.parent, "crash-", ".tmp")
        try {
            FileChannel.open(temporary, StandardOpenOption.WRITE).use { channel ->
                val bytes = ByteBuffer.wrap(data)
                while (bytes.hasRemaining()) channel.write(bytes)
                channel.force(true)
            }
            Files.move(temporary, file, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { Files.deleteIfExists(temporary) }
    }
    companion object {
        private const val MAX_CRASH_BYTES = 32768
        private const val MAGIC = 0x50435231
        private val identity = Regex("[A-Za-z0-9_.$<>]{1,160}")
        private fun codeIdentity(value: String) = value.takeIf { identity.matches(it) } ?: "unavailable"
        internal fun encodeCrash(error: Throwable, atMs: Long): ByteArray {
            val causes = mutableListOf<Throwable>()
            val seen = Collections.newSetFromMap(IdentityHashMap<Throwable, Boolean>())
            var next: Throwable? = error
            while (next != null && causes.size < 4 && seen.add(next)) { causes += next; next = next.cause }
            return ByteArrayOutputStream().use { bytes ->
                DataOutputStream(bytes).use { out ->
                    out.writeInt(MAGIC); out.writeLong(atMs); out.writeInt(causes.size)
                    causes.forEach { cause ->
                        out.writeUTF(codeIdentity(cause.javaClass.name))
                        val frames = cause.stackTrace.take(24)
                        out.writeInt(frames.size)
                        frames.forEach { frame -> out.writeUTF(codeIdentity(frame.className)); out.writeUTF(codeIdentity(frame.methodName)); out.writeInt(frame.lineNumber) }
                    }
                }
                bytes.toByteArray().also { require(it.size <= MAX_CRASH_BYTES) }
            }
        }
        internal fun decodeCrash(data: ByteArray): String {
            require(data.size <= MAX_CRASH_BYTES)
            return DataInputStream(ByteArrayInputStream(data)).use { input ->
                require(input.readInt() == MAGIC)
                val at = input.readLong(); val count = input.readInt(); require(count in 1..4)
                val text = buildString {
                    appendLine("At epoch ms: $at")
                    repeat(count) {
                        val kind = input.readUTF(); require(identity.matches(kind)); appendLine("Exception class: $kind")
                        val frames = input.readInt(); require(frames in 0..24)
                        repeat(frames) {
                            val type = input.readUTF(); val method = input.readUTF(); val line = input.readInt()
                            require(identity.matches(type) && identity.matches(method))
                            appendLine("  at $type.$method:$line")
                        }
                    }
                }
                require(input.read() == -1)
                text
            }
        }
    }
}
