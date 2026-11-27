package io.github.stratayun.paranoic.platform

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.stratayun.paranoic.nativecore.NativeGrant
import java.io.File
import java.security.KeyStore
import java.util.UUID
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.test.*

/** Synthetic grants, a fresh noBackup subdirectory and a unique TEST key only. */
@RunWith(AndroidJUnit4::class)
class AndroidGrantStoreTest {
    private lateinit var directory: File
    private lateinit var alias: String
    private val keys get() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private val encrypted get() = File(directory, "grants-v1.enc")
    private val fixture = listOf(NativeGrant("7b4b2f90-5de5-4df8-b5a5-04b674955e02",
        "/storage/fixture/test-only"))

    @Before fun setUp() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val id = UUID.randomUUID().toString()
        alias = "paranoic-test-$id"
        directory = File(context.noBackupFilesDir, "test-grants-$id")
        check(directory.mkdir())
    }
    @After fun tearDown() {
        if (::alias.isInitialized) {
            check(alias.startsWith("paranoic-test-"))
            keys.deleteEntry(alias)
        }
        if (::directory.isInitialized) directory.deleteRecursively()
    }

    @Test fun emptyReadDoesNotCreateKeyOrFile() {
        assertTrue(AndroidGrantStore(directory, alias).read().isEmpty())
        assertFalse(keys.containsAlias(alias))
        assertFalse(encrypted.exists())
    }

    @Test fun encryptedRoundTripSurvivesStoreRecreation() {
        AndroidGrantStore(directory, alias).write(fixture)
        val bytes = encrypted.readBytes()
        assertFalse(bytes.toString(Charsets.ISO_8859_1).contains(fixture.single().reference))
        val loaded = AndroidGrantStore(directory, alias).read().single()
        assertEquals(fixture.single().id, loaded.id)
        assertEquals(fixture.single().reference, loaded.reference)
    }

    @Test fun ciphertextTamperingFailsWithoutReplacingEvidence() {
        AndroidGrantStore(directory, alias).write(fixture)
        val corrupt = encrypted.readBytes().also { it[it.lastIndex] = (it.last().toInt() xor 1).toByte() }
        encrypted.writeBytes(corrupt)
        assertFails { AndroidGrantStore(directory, alias).read() }
        assertContentEquals(corrupt, encrypted.readBytes())
    }

    @Test fun missingKeyNeverSilentlyReplacesExistingConnections() {
        AndroidGrantStore(directory, alias).write(fixture)
        val original = encrypted.readBytes()
        keys.deleteEntry(alias)
        val store = AndroidGrantStore(directory, alias)
        assertFails { store.read() }
        assertFails { store.write(emptyList()) }
        assertFalse(keys.containsAlias(alias))
        assertContentEquals(original, encrypted.readBytes())
    }

    @Test fun oversizedAndTruncatedFilesDoNotCreateAReplacementKey() {
        val store = AndroidGrantStore(directory, alias)
        for (size in listOf(0, 28, 2 * 1024 * 1024 + 1)) {
            val bytes = ByteArray(size) { 0x55 }
            encrypted.writeBytes(bytes)
            assertFails("size=$size") { store.read() }
            assertFalse(keys.containsAlias(alias))
            assertContentEquals(bytes, encrypted.readBytes())
        }
    }

    @Test fun interruptedAtomicWriteRecoversPreviousEncryptedSnapshot() {
        AndroidGrantStore(directory, alias).write(fixture)
        val original = encrypted.readBytes()
        check(encrypted.renameTo(File(encrypted.path + ".bak")))
        encrypted.writeBytes(byteArrayOf(1, 2, 3))
        val loaded = AndroidGrantStore(directory, alias).read()
        assertEquals(fixture.single().reference, loaded.single().reference)
        assertContentEquals(original, encrypted.readBytes())
    }
}
