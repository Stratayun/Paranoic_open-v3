package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.runBlocking
import kotlin.test.*

class EncryptedFolderPolicyTest {
    @Test fun metadataFailureOrEitherReadOnlyPolicyCannotGrantWrites() {
        val readOnly = SavedVault(Location("disk", "vault"), VaultKind.Paranoic, "Vault", readOnly = true)
        assertFalse(canWriteOpenedVault(true, readOnly, true, false))
        assertFalse(canWriteOpenedVault(true, null, false, false))
        assertFalse(canWriteOpenedVault(true, null, true, true))
        assertFalse(canWriteOpenedVault(false, null, true, false))
        assertTrue(canWriteOpenedVault(true, null, true, false))
    }
    @Test fun restrictedHandleRejectsEveryMutationBeforeCallingCore() = runBlocking {
        for (format in FolderFormat.entries) {
            val gateway = EncryptedFolderGateway(DisconnectedCoreGateway, format)
            gateway.restrictWrites("readonly")
            val source = Location("disk", "source")
            val incoming = object : IncomingDocument {
                override val id = "one-shot"
                override val name = "incoming.txt"
                override val size = 1uL
                override val modifiedMs: ULong? = null
                override fun open(): IncomingByteSource = error("read-only must not open incoming content")
                override fun discard() = error("the caller still owns a refused capability")
            }
            val results = listOf(
                gateway.mkdir("readonly", emptyList(), "new"),
                gateway.rename("readonly", emptyList(), "old", "new", false),
                gateway.delete("readonly", emptyList(), "file", false),
                gateway.add("readonly", emptyList(), source),
                gateway.moveIn("readonly", emptyList(), source, "fixed-request"),
                gateway.addIncoming("readonly", emptyList(), "incoming.txt", incoming),
                gateway.textSave("readonly", emptyList(), "entry.txt", VaultTextVersion(1uL, null, "opaque-token"), "draft"),
                gateway.textCreate("readonly", emptyList(), "new.txt", "draft"),
                gateway.archiveExtract("readonly", emptyList(), "input.zip", ArchiveFormat.Zip, null, emptyList(), "new-folder"),
                gateway.move("readonly", emptyList(), VaultEntry("entry", false, 0uL, 0), listOf("other")),
                gateway.replace("readonly", emptyList(), "entry", source),
                gateway.setRecovery("readonly", "password"),
                gateway.resetPassword("readonly", "key", "new password"),
                gateway.changePassword("readonly", "old", "new"))
            assertTrue(results.all { it is CoreResult.Failure && it.fault == CoreFault.Permission })
            // The disconnected backend must still be reached for unrelated sessions and reads.
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.mkdir("other", emptyList(), "new"))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.list("readonly", emptyList()))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.export("readonly", emptyList(), "entry", source))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.openStream("readonly", emptyList(), "entry"))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.textOpen("readonly", emptyList(), "entry.txt"))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.archiveList("readonly", emptyList(), "entry.zip", ArchiveFormat.Zip, null))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.pdfPage("readonly", emptyList(), "entry.pdf", 0, 2048))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.pdfSearch("readonly", emptyList(), "entry.pdf", "query", 0))
            assertNotEquals(CoreResult.Failure(CoreFault.Permission), gateway.officeOpen("readonly", emptyList(), "entry.docx"))
        }
    }
    @Test fun restrictionIsRetainedIfCoreCannotConfirmLock() = runBlocking {
        val gateway = EncryptedFolderGateway(DisconnectedCoreGateway, FolderFormat.Paranoic)
        gateway.restrictWrites("readonly")
        assertTrue(gateway.lock("readonly") is CoreResult.Failure)
        assertEquals(CoreResult.Failure(CoreFault.Permission), gateway.mkdir("readonly", emptyList(), "new"))
    }
}
