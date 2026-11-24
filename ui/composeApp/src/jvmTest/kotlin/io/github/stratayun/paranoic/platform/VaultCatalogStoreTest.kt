package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import kotlin.test.*

class VaultCatalogStoreTest {
    // A deterministic test envelope, not a cryptographic implementation.
    private val envelope = object : CatalogProtection {
        override fun seal(data: ByteArray, createKey: Boolean) = byteArrayOf(91) + data
        override fun open(data: ByteArray): ByteArray {
            require(data.firstOrNull() == 91.toByte())
            return data.copyOfRange(1, data.size)
        }
    }
    @Test fun roundtripRenameRemovalAndUndo() = runBlocking {
        val directory = Files.createTempDirectory("paranoic-catalog-test-")
        val path = directory.resolve("catalog")
        try {
            val store = ProtectedVaultCatalogStore(path, envelope)
            assertEquals(VaultCatalogSnapshot(), store.read())
            val entry = SavedVault(Location("grant", "nested/vault"), VaultKind.Cryptomator, "Личное")
            val initial = VaultCatalogSnapshot(listOf(entry)); store.write(initial)
            assertEquals(initial, store.read())
            val renamed = initial.rename(entry.location, "Work"); store.write(renamed)
            assertEquals("Work", store.read().entries.single().title)
            store.write(renamed.remove(entry.location)); assertTrue(store.read().entries.isEmpty())
            store.write(store.read().restore(renamed.entries.single(), 0)); assertEquals(renamed, store.read())
            assertEquals(1L, Files.list(directory).use { it.count() })
        } finally { Files.deleteIfExists(path); Files.deleteIfExists(directory) }
    }
    @Test fun malformedOrUnauthenticatedStoreIsNotEmpty() = runBlocking {
        val directory = Files.createTempDirectory("paranoic-catalog-test-"); val path = directory.resolve("catalog")
        try {
            val store = ProtectedVaultCatalogStore(path, envelope)
            for (bad in listOf(byteArrayOf(0), byteArrayOf(91, 0, 1, 2))) {
                Files.write(path, bad)
                assertFails { store.read() }
                assertContentEquals(bad, Files.readAllBytes(path))
            }
        } finally { Files.deleteIfExists(path); Files.deleteIfExists(directory) }
    }
    @Test fun previousV1PayloadRemainsReadableAndWritable() = runBlocking {
        val directory = Files.createTempDirectory("paranoic-catalog-v1-"); val path = directory.resolve("catalog")
        try {
            val payload = ByteArrayOutputStream().use { bytes ->
                DataOutputStream(bytes).use { out ->
                    out.writeInt(0x50564331); out.writeInt(1)
                    out.writeUTF("grant"); out.writeUTF("old/vault"); out.writeByte(1); out.writeUTF("Existing")
                }
                byteArrayOf(91) + bytes.toByteArray()
            }
            Files.write(path, payload)
            val store = ProtectedVaultCatalogStore(path, envelope)
            val expected = VaultCatalogSnapshot(listOf(SavedVault(Location("grant", "old/vault"), VaultKind.Cryptomator, "Existing")))
            assertEquals(expected, store.read())
            assertContentEquals(payload, Files.readAllBytes(path)) // Reading does not migrate or rewrite.
            store.write(expected)
            assertEquals(expected, store.read())
            assertEquals(0x50564332, java.io.DataInputStream(java.io.ByteArrayInputStream(Files.readAllBytes(path).drop(1).toByteArray())).readInt())
        } finally { Files.deleteIfExists(path); Files.deleteIfExists(directory) }
    }
    @Test fun accessSettingsRoundtripInV2() = runBlocking {
        val directory = Files.createTempDirectory("paranoic-catalog-settings-"); val path = directory.resolve("catalog")
        try {
            val entry = SavedVault(Location("grant", "vault"), VaultKind.Paranoic, "Work", true, 60)
            val expected = VaultCatalogSnapshot(listOf(entry), 900)
            val store = ProtectedVaultCatalogStore(path, envelope)
            store.write(expected); assertEquals(expected, store.read())
        } finally { Files.deleteIfExists(path); Files.deleteIfExists(directory) }
    }
}
