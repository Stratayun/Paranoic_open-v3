package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.bridge.Location
import io.github.stratayun.paranoic.bridge.VaultKind
import java.io.*
import java.nio.file.Path

/** Reads v1/v2 with the existing OS key. Writes v2 only after an explicit change. */
internal class ProtectedVaultCatalogStore(file: Path, protection: CatalogProtection) : VaultCatalogStore {
    private val metadata = ProtectedMetadataFile(file, protection)
    override suspend fun read(): VaultCatalogSnapshot {
        val plain = metadata.read() ?: return VaultCatalogSnapshot()
        try { return decode(plain) } finally { plain.fill(0) }
    }
    override suspend fun write(snapshot: VaultCatalogSnapshot) {
        val plain = encode(snapshot)
        try { metadata.write(plain) } finally { plain.fill(0) }
    }
    companion object {
        const val MAX_BYTES = 1024 * 1024
        private fun encode(snapshot: VaultCatalogSnapshot): ByteArray = ByteArrayOutputStream().use { bytes ->
            DataOutputStream(bytes).use { out ->
                out.writeInt(0x50564332); out.writeInt(snapshot.entries.size)
                snapshot.entries.forEach { entry ->
                    out.writeUTF(entry.location.storageId); out.writeUTF(entry.location.path)
                    out.writeByte(if (entry.kind == VaultKind.Paranoic) 0 else 1); out.writeUTF(entry.title)
                    out.writeByte(if (entry.readOnly) 1 else 0); out.writeInt(entry.idleLockSeconds ?: 0)
                }
                out.writeInt(snapshot.defaultIdleLockSeconds)
            }
            bytes.toByteArray()
        }
        private fun decode(plain: ByteArray): VaultCatalogSnapshot {
            require(plain.size <= MAX_BYTES)
            return DataInputStream(ByteArrayInputStream(plain)).use { input ->
                val version = input.readInt(); require(version == 0x50564331 || version == 0x50564332)
                val count = input.readInt(); require(count in 0..128)
                val entries = List(count) {
                    val storage = input.readUTF(); val path = input.readUTF()
                    val kind = when (input.readUnsignedByte()) { 0 -> VaultKind.Paranoic; 1 -> VaultKind.Cryptomator; else -> error("Invalid vault format") }
                    val title = input.readUTF()
                    val readOnly = if (version == 0x50564332) when (input.readUnsignedByte()) { 0 -> false; 1 -> true; else -> error("Invalid access mode") } else false
                    val seconds = if (version == 0x50564332) input.readInt().let { if (it == 0) null else it } else null
                    SavedVault(Location(storage, path), kind, title, readOnly, seconds)
                }
                val defaultSeconds = if (version == 0x50564332) input.readInt() else 300
                require(input.read() == -1)
                VaultCatalogSnapshot(entries, defaultSeconds)
            }
        }
    }
}
