package io.github.stratayun.paranoic.platform

import android.Manifest
import android.content.pm.PackageManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.nativecore.*
import kotlinx.coroutines.runBlocking
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID
import kotlin.test.*

/** Real packaged UniFFI/JNA/Rust plus a dedicated Keystore alias. Never uses
 * production notes, grants, the journal key or a user's storage. */
@RunWith(AndroidJUnit4::class)
class NativeFeaturesTest {
    private fun <T> CoreResult<T>.value(): T = (this as? CoreResult.Value<T>)?.value
        ?: error("Native call failed: $this")

    @Test fun packagedBridgeNotesClearAndVaultRoundTrip() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertEquals(PackageManager.PERMISSION_GRANTED, context.checkSelfPermission(Manifest.permission.INTERNET))
        val root = File(context.noBackupFilesDir, "test-native-${UUID.randomUUID()}").apply { check(mkdir()) }
        val key = AndroidNotesKey("paranoic-test-notes-${UUID.randomUUID()}")
        val store = object : NativeGrantStore {
            private var saved = emptyList<NativeGrant>()
            override fun read() = saved
            override fun write(grants: List<NativeGrant>) { saved = grants }
        }
        // Journal confidentiality is covered separately; no production alias here.
        val sealer = object : RecordSealer {
            override fun established() = false
            override fun available() = false
            override fun seal(data: ByteArray) = HostBytes(2u, byteArrayOf())
            override fun open(data: ByteArray) = HostBytes(2u, byteArrayOf())
        }
        var gateway: NativeCoreGateway? = null
        try {
            val workspace = File(root, "files")
            val core = NativeCoreGateway.open(File(root, "state").absolutePath, workspace.absolutePath,
                "", "", store, sealer, notesKey = key)
            gateway = core
            val password = "correct horse battery staple 42"
            core.notesCreate(password).value()
            val book = listOf(NoteTab("tab-1", "Keys", listOf(Note("note-1", "Test", "private marker", 1))))
            core.notesSave(book).value()
            assertEquals(book, core.notesRead().value())
            // StrongBox backing alone must never claim rollback-resistant erasure.
            assertEquals(KeyGuarantee.DeviceKey, core.notesStatus().value().guarantee)
            core.lockSecrets()
            assertFalse(core.notesStatus().value().unlocked)
            assertEquals(CoreResult.Failure(CoreFault.Locked), core.notesRead())
            core.notesUnlock(password).value()
            assertEquals(book, core.notesRead().value())
            val notesDir = File(root, "state/notes")
            val copy = notesDir.listFiles()!!.filter { it.name.length == 32 }.associate { it.name to it.readBytes() }
            core.notesClear().value()
            assertFalse(core.notesStatus().value().setUp)
            copy.forEach { (name, bytes) -> File(notesDir, name).writeBytes(bytes) }
            assertEquals(CoreResult.Failure(CoreFault.DeviceKey), core.notesUnlock(password), "old key record reopened after Clear")

            File(workspace, "source.txt").writeText("vault marker")
            File(workspace, "out").mkdir()
            val app = core.spaces().value().first()
            val handle = core.vaultCreate(app.root, "Vault", password).value()
            // A hostile, not-yet-authenticated header cannot request 1 GiB on
            // Android. Admission must fail before running that derivation.
            val header = File(workspace, "Vault/paranoic.vault")
            val original = header.readBytes()
            val costly = original.copyOf()
            val memoryKiB = 1024 * 1024
            for (i in 0..3) costly[28 + i] = (memoryKiB ushr (i * 8)).toByte()
            header.writeBytes(costly)
            try { assertEquals(CoreResult.Failure(CoreFault.Limit), core.vaultOpen(Location(app.id, "Vault"), password)) }
            finally { header.writeBytes(original) }
            core.vaultAdd(handle, emptyList(), Location(app.id, "source.txt")).value()
            assertEquals("source.txt", core.vaultList(handle, emptyList()).value().entries.single().name)
            core.vaultExport(handle, emptyList(), "source.txt", Location(app.id, "out")).value()
            assertEquals("vault marker", File(workspace, "out/source.txt").readText())
            assertIs<CoreResult.Failure>(core.vaultExport(handle, emptyList(), "source.txt", Location(app.id, "out")))
            core.lockSecrets()
            core.notesStatus().value() // waits for the asynchronous native lock
            assertEquals(CoreResult.Failure(CoreFault.Locked), core.vaultList(handle, emptyList()))
        } finally {
            gateway?.shutdown()
            key.eraseKey()
            root.deleteRecursively()
        }
    }
}
