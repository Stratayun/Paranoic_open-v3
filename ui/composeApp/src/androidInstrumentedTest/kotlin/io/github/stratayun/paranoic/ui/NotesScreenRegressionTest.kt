package io.github.stratayun.paranoic.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.screens.notes.NotesScreen
import io.github.stratayun.paranoic.ui.state.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** Isolated UI cases: no native Core, personal notes, disk writes or cloud. */
@RunWith(AndroidJUnit4::class)
class NotesScreenRegressionTest {
    @get:Rule val compose = createComposeRule()

    private class NotesCore : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var notebook = listOf(NoteTab("tab", "Existing tab", listOf(Note("note", "Title", "Body", 1,
            listOf(NoteAttachment("old-attachment", "example.txt", 3))))))
        var rotated = false
        var failReload = false
        val calls = mutableListOf<String>()
        override suspend fun notesStatus() = CoreResult.Value(
            NotesStatus(true, true, false, 0, 10, 0, KeyGuarantee.PasswordOnly))
        override suspend fun notesPasswordStrength(password: String) = CoreResult.Value(PasswordStrength.Strong)
        override suspend fun passwordFeedback(password: String) = CoreResult.Value(PasswordFeedback(PasswordStrength.Strong, emptyList()))
        override suspend fun notesRead(): CoreResult<List<NoteTab>> {
            calls += "read"
            return if (rotated && failReload) CoreResult.Failure(CoreFault.Unavailable) else CoreResult.Value(notebook)
        }
        override suspend fun notesSave(tabs: List<NoteTab>): CoreResult<Unit> {
            calls += "save"
            if (rotated) assertTrue(tabs.flatMap { it.notes }.flatMap { it.attachments }.all { it.id == "new-attachment" })
            notebook = tabs
            return CoreResult.Value(Unit)
        }
        override suspend fun notesChangePassword(old: String, new: String): CoreResult<Unit> {
            calls += "change"
            rotated = true
            notebook = notebook.map { tab -> tab.copy(notes = tab.notes.map { note ->
                note.copy(attachments = note.attachments.map { it.copy(id = "new-attachment") })
            }) }
            return CoreResult.Value(Unit)
        }
    }

    private fun show(core: NotesCore) {
        compose.setContent {
            val scope = rememberCoroutineScope()
            val session = remember { AppSession(core, scope) }
            CompositionLocalProvider(LocalUiLanguage provides UiLanguage.English) {
                MaterialTheme { NotesScreen(session) }
            }
        }
        compose.waitForIdle()
    }

    private fun changePassword() {
        compose.onNodeWithText("Notes settings", substring = true).performScrollTo().performClick()
        compose.onNodeWithText("Change password").performScrollTo().performClick()
        compose.onNodeWithText("Current password").performTextReplacement("old-password-123")
        compose.onNodeWithText("New password").performTextReplacement("new-password-456")
        compose.onNodeWithText("Repeat new password").performTextReplacement("new-password-456")
        compose.onNodeWithText("Change", substring = false).performClick()
        compose.waitForIdle()
    }

    @Test fun firstAndSubsequentTabsCanBeCreatedSelectedAndRemoved() {
        val core = NotesCore().apply { notebook = emptyList() }
        show(core)
        compose.onNodeWithText("+ Tab").performScrollTo().performClick()
        compose.onNodeWithText("Tab 1").assertIsSelected()
        compose.onNodeWithText("+ Tab").performScrollTo().performClick()
        compose.onNodeWithText("Tab 2").assertIsSelected()
        compose.onNodeWithText("Tab 1").performClick().assertIsSelected()
        compose.onNodeWithText("Delete tab").performScrollTo().performClick()
        compose.onNodeWithText("Delete", substring = false).performClick()
        compose.onNodeWithText("Tab 2").assertIsSelected()
        compose.onNodeWithText("Save", substring = false).performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("Tab 2"), core.notebook.map { it.title }) }
    }

    @Test fun passwordChangeReloadsAttachmentIdsBeforeNextSave() {
        val core = NotesCore()
        show(core)
        changePassword()
        compose.runOnIdle { assertEquals(listOf("read", "save", "change", "read"), core.calls) }
        // The next edit must save the attachment returned by the post-rotation read.
        compose.onNodeWithText("+ Tab").performScrollTo().performClick()
        compose.onNodeWithText("Save", substring = false).performScrollTo().performClick()
        compose.runOnIdle { assertEquals("new-attachment", core.notebook.first().notes.single().attachments.single().id) }
    }

    @Test fun failedReloadAfterPasswordChangeCannotSaveStaleAttachmentIds() {
        val core = NotesCore().apply { failReload = true }
        show(core)
        changePassword()
        compose.onNodeWithText("Load notes").assertExists()
        compose.onNodeWithText("Save", substring = false).assertDoesNotExist()
        compose.onNodeWithText("+ Tab").assertDoesNotExist()
        compose.runOnIdle { assertEquals(listOf("read", "save", "change", "read"), core.calls) }
    }
}
