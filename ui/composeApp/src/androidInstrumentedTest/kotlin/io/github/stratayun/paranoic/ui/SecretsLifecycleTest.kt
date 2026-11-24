package io.github.stratayun.paranoic.ui

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CompletableDeferred
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Recomposition must discard the whole secret screen, including dialogs and
 * passwords, before a delayed status response comes back after Quick Lock. */
@RunWith(AndroidJUnit4::class)
class SecretsLifecycleTest {
    @get:Rule val compose = createComposeRule()

    @Test fun quickLockDropsSecretDialogsBeforeStatusReply() {
        var locked = false
        val delayedStatus = CompletableDeferred<CoreResult<NotesStatus>>()
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun notesStatus(): CoreResult<NotesStatus> = if (locked) delayedStatus.await()
                else CoreResult.Value(NotesStatus(true, true, false, 0, 10, 0, KeyGuarantee.PasswordOnly))
            override suspend fun notesRead() = CoreResult.Value(listOf(NoteTab("t", "Test tab",
                listOf(Note("n", "secret title", "secret body", 1)))))
            override fun lockSecrets() { locked = true }
        }
        val preferences = object : AppearanceStore {
            override fun read() = AppearanceSettings(language = UiLanguage.English)
            override fun write(settings: AppearanceSettings) = true
        }
        lateinit var session: AppSession
        compose.setContent {
            ParanoicApp(core, preferences, folderPicker = { {} }, pngImage = { _, _ -> }, mediaPlayer = { _, _, _, _ -> },
                sessionLifecycle = { session = it })
        }
        compose.onNodeWithText("Notes").performClick()
        compose.onNodeWithText("secret title").assertExists()
        compose.onNodeWithText("Notes settings", substring = true).performClick() // label carries a ▸/▾ marker
        compose.onNodeWithText("Change password").performScrollTo().performClick()
        compose.onNodeWithText("Current password").performTextReplacement("password-to-forget")
        compose.runOnIdle { session.pause() }
        compose.onNodeWithText("Current password").assertDoesNotExist()
        compose.onNodeWithText("secret title").assertDoesNotExist()
        compose.onNodeWithText("password-to-forget").assertDoesNotExist()
        delayedStatus.complete(CoreResult.Value(NotesStatus(true, false, false, 0, 10, 0, KeyGuarantee.PasswordOnly)))
        compose.onNodeWithText("Notes are locked").assertExists()
    }
}
