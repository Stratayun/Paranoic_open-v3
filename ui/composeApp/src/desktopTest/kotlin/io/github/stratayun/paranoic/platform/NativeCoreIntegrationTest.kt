package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.NativeGrant
import io.github.stratayun.paranoic.nativecore.RecordSealer
import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import kotlin.test.*

/** Loads the packaged host library, not a fake CoreGateway: generated UniFFI,
 * JNA, Rust Core, notes and the vault run together on disposable data. */
class NativeCoreIntegrationTest {
    private fun <T> CoreResult<T>.value(): T = (this as? CoreResult.Value<T>)?.value
        ?: error("Native call failed: $this")

    @Test fun packagedRuntimeNotesAndVaultRoundTrip() = runBlocking {
        val root = Files.createTempDirectory("paranoic-native-test-").toFile()
        val store = object : NativeGrantStore {
            override fun read() = emptyList<NativeGrant>()
            override fun write(grants: List<NativeGrant>) = Unit
        }
        val sealer = object : RecordSealer {
            override fun available() = false
            override fun seal(data: ByteArray) = HostBytes(2u, byteArrayOf())
            override fun open(data: ByteArray) = HostBytes(2u, byteArrayOf())
        }
        var gateway: NativeCoreGateway? = null
        try {
            val workspace = root.resolve("files")
            val core = NativeCoreGateway.open(root.resolve("state").absolutePath, workspace.absolutePath,
                "", "", NoAndroidDocuments, store, sealer)
            gateway = core
            val password = "Correct horse battery staple 42!"
            // Policy: lower, upper, digit and symbol. A phrase without them is refused before any state exists.
            assertEquals(CoreResult.Failure(CoreFault.WeakPassword), core.notesCreate("correct horse battery staple 42"))
            assertFalse(core.notesStatus().value().setUp)
            core.notesCreate(password).value()
            val notes = listOf(NoteTab("t", "Test", listOf(Note("n", "Title", "private marker", 1))))
            core.notesSave(notes).value()
            assertEquals(notes, core.notesRead().value())
            core.lockSecrets()
            assertFalse(core.notesStatus().value().unlocked)
            assertEquals(CoreResult.Failure(CoreFault.Locked), core.notesRead())
            core.notesUnlock(password).value()
            assertEquals(notes, core.notesRead().value())
            assertEquals(KeyGuarantee.PasswordOnly, core.notesClear().value())
            assertFalse(core.notesStatus().value().setUp)
            core.notesCreate(password).value()
            assertTrue(core.notesRead().value().isEmpty())

            workspace.resolve("source.txt").writeText("vault marker")
            workspace.resolve("out").mkdir()
            val app = core.spaces().value().first()
            val handle = core.vaultCreate(app.root, "Vault", password).value()
            core.vaultAdd(handle, emptyList(), Location(app.id, "source.txt")).value()
            assertEquals("source.txt", core.vaultList(handle, emptyList()).value().entries.single().name)
            core.vaultExport(handle, emptyList(), "source.txt", Location(app.id, "out")).value()
            assertEquals("vault marker", workspace.resolve("out/source.txt").readText())
            assertIs<CoreResult.Failure>(core.vaultExport(handle, emptyList(), "source.txt", Location(app.id, "out")))
            core.lockSecrets()
            assertEquals(CoreResult.Failure(CoreFault.Locked), core.vaultList(handle, emptyList()))
            assertTrue(workspace.resolve("source.txt").exists(), "source removed during encryption")
        } finally {
            gateway?.shutdown()
            root.deleteRecursively()
        }
    }
}
