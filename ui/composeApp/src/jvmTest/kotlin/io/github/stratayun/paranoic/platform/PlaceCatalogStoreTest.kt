package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.runBlocking
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

class PlaceCatalogStoreTest {
    // Deterministic test envelope; Android Keystore and Windows DPAPI are not mocked as verified.
    private val envelope = object : CatalogProtection {
        override fun seal(data: ByteArray, createKey: Boolean) = byteArrayOf(91) + data
        override fun open(data: ByteArray): ByteArray {
            require(data.firstOrNull() == 91.toByte())
            return data.copyOfRange(1, data.size)
        }
    }
    private suspend fun inDirectory(block: suspend (Path, Path) -> Unit) {
        val dir = Files.createTempDirectory("paranoic-places-")
        try { block(dir, dir.resolve("places")) }
        finally {
            Files.list(dir).use { entries -> entries.forEach { Files.deleteIfExists(it) } }
            Files.deleteIfExists(dir)
        }
    }
    @Test fun roundtripAndUndoKeepNavigationPreferences() = runBlocking {
        inDirectory { dir, path ->
            val store = ProtectedPlaceCatalogStore(path, envelope)
            assertEquals(PlaceCatalogSnapshot(), store.read())
            val named = PlaceCatalogSnapshot().rename("grant", "Личное")
            val (hidden, removal) = named.hide("grant")
            store.write(hidden); assertEquals(hidden, store.read())
            store.write(store.read().undo(removal)); assertEquals(named, store.read())
            assertEquals(1L, Files.list(dir).use { it.count() })
        }
    }
    @Test fun pinnedFoldersRoundtripAndOldFormatStillReads() = runBlocking {
        inDirectory { _, path ->
            val store = ProtectedPlaceCatalogStore(path, envelope)
            val old = ByteArrayOutputStream().use { bytes ->
                DataOutputStream(bytes).use { out -> out.writeInt(0x50504331); out.writeInt(1); out.writeUTF("grant"); out.writeByte(0); out.writeByte(1) }
                byteArrayOf(91) + bytes.toByteArray()
            }
            Files.write(path, old)
            assertEquals(PlaceCatalogSnapshot(listOf(PlacePreference("grant", hidden = true))), store.read())
            val pinned = store.read().pin(PlaceBookmark("grant", "Documents/Work", "Work"))
            assertSame(pinned, pinned.pin(PlaceBookmark("grant", "Documents/Work", "Again")), "same folder pinned once")
            store.write(pinned); assertEquals(pinned, store.read())
            store.write(pinned.unpin("grant", "Documents/Work")); assertTrue(store.read().bookmarks.isEmpty())
        }
    }
    @Test fun corruptionDuplicateIdsAndTrailingBytesDoNotResetTheStore() = runBlocking {
        inDirectory { _, path ->
            val duplicate = ByteArrayOutputStream().use { bytes ->
                DataOutputStream(bytes).use { out ->
                    out.writeInt(0x50504331); out.writeInt(2)
                    repeat(2) { out.writeUTF("same"); out.writeByte(0); out.writeByte(0) }
                }
                byteArrayOf(91) + bytes.toByteArray()
            }
            val store = ProtectedPlaceCatalogStore(path, envelope)
            store.write(PlaceCatalogSnapshot().rename("id", "Title"))
            val valid = Files.readAllBytes(path)
            for (bad in listOf(byteArrayOf(0), byteArrayOf(91, 1, 2), duplicate, valid + 0.toByte())) {
                Files.write(path, bad)
                assertFails { store.read() }
                assertContentEquals(bad, Files.readAllBytes(path))
            }
        }
    }
    @Test fun rejectedProtectionKeepsThePreviousFileAndLeavesNoTempFile() = runBlocking {
        inDirectory { dir, path ->
            val store = ProtectedPlaceCatalogStore(path, envelope)
            store.write(PlaceCatalogSnapshot().rename("id", "Original"))
            val previous = Files.readAllBytes(path)
            val failing = ProtectedPlaceCatalogStore(path, object : CatalogProtection {
                override fun seal(data: ByteArray, createKey: Boolean): ByteArray = error("Key unavailable")
                override fun open(data: ByteArray) = envelope.open(data)
            })
            assertFails { failing.write(PlaceCatalogSnapshot().rename("id", "Changed")) }
            assertContentEquals(previous, Files.readAllBytes(path))
            assertEquals(1L, Files.list(dir).use { it.count() })
        }
    }
}
