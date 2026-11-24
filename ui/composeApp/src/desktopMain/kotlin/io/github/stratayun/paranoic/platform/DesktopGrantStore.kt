package io.github.stratayun.paranoic.platform

import com.sun.jna.platform.win32.Crypt32Util
import io.github.stratayun.paranoic.nativecore.NativeGrant
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.channels.FileChannel
import java.nio.file.StandardOpenOption

/** DPAPI current-user scope. A corrupt store is an error, never reset silently. */
class DesktopGrantStore(private val root: Path) : NativeGrantStore {
    private val file = root.resolve("grants-v1.dpapi")
    @Synchronized override fun read(): List<NativeGrant> {
        if (!Files.exists(file)) return emptyList()
        require(Files.size(file) <= GrantCodec.MAX_BYTES)
        val plain = Crypt32Util.cryptUnprotectData(Files.readAllBytes(file))
        return try { GrantCodec.decode(plain) } finally { plain.fill(0) }
    }
    @Synchronized override fun write(grants: List<NativeGrant>) {
        if (!System.getProperty("os.name").startsWith("Windows")) throw UnsupportedOperationException("DPAPI requires Windows")
        Files.createDirectories(root)
        val plain = GrantCodec.encode(grants)
        val data = try { Crypt32Util.cryptProtectData(plain) } finally { plain.fill(0) }
        // DPAPI overhead is variable; validate the actual stored bytes before
        // creating a replacement so a rejected write preserves the old store.
        require(data.size <= GrantCodec.MAX_BYTES) { "Encrypted grant store exceeds byte limit" }
        val temporary = Files.createTempFile(root, "grants-", ".tmp")
        try {
            FileChannel.open(temporary, StandardOpenOption.WRITE).use { channel ->
                val buffer = java.nio.ByteBuffer.wrap(data)
                while (buffer.hasRemaining()) channel.write(buffer)
                channel.force(true)
            }
            Files.move(temporary, file, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { Files.deleteIfExists(temporary) }
    }
}


