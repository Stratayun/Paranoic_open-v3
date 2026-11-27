package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import java.io.*
import java.nio.file.Path

internal class ProtectedPlaceCatalogStore(file: Path, protection: CatalogProtection) : PlaceCatalogStore {
    private val metadata = ProtectedMetadataFile(file, protection)
    override suspend fun read(): PlaceCatalogSnapshot {
        val plain = metadata.read() ?: return PlaceCatalogSnapshot()
        try {
            return DataInputStream(ByteArrayInputStream(plain)).use { input ->
                // PPC1: places only; PPC2 adds pinned folders (owner 27.11).
                val magic = input.readInt(); require(magic == 0x50504331 || magic == 0x50504332)
                val count = input.readInt(); require(count in 0..256)
                val entries = List(count) {
                    val id = input.readUTF()
                    val title = when (input.readUnsignedByte()) { 0 -> null; 1 -> input.readUTF(); else -> error("Invalid title flag") }
                    val hidden = when (input.readUnsignedByte()) { 0 -> false; 1 -> true; else -> error("Invalid visibility flag") }
                    PlacePreference(id, title, hidden)
                }
                val bookmarks = if (magic == 0x50504332) {
                    val pinned = input.readInt(); require(pinned in 0..MAX_BOOKMARKS)
                    List(pinned) { PlaceBookmark(input.readUTF(), input.readUTF(), input.readUTF()) }
                } else emptyList()
                require(input.read() == -1)
                PlaceCatalogSnapshot(entries, bookmarks)
            }
        } finally { plain.fill(0) }
    }
    override suspend fun write(snapshot: PlaceCatalogSnapshot) {
        val plain = ByteArrayOutputStream().use { bytes ->
            DataOutputStream(bytes).use { out ->
                out.writeInt(0x50504332); out.writeInt(snapshot.entries.size)
                snapshot.entries.forEach { entry ->
                    out.writeUTF(entry.storageId)
                    out.writeByte(if (entry.title == null) 0 else 1)
                    entry.title?.let { out.writeUTF(it) }
                    out.writeByte(if (entry.hidden) 1 else 0)
                }
                out.writeInt(snapshot.bookmarks.size)
                snapshot.bookmarks.forEach { out.writeUTF(it.storageId); out.writeUTF(it.path); out.writeUTF(it.title) }
            }
            bytes.toByteArray()
        }
        try { metadata.write(plain) } finally { plain.fill(0) }
    }
}
