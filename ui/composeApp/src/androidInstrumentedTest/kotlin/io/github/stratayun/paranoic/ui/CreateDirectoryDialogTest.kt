package io.github.stratayun.paranoic.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextReplacement
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.stratayun.paranoic.ui.components.CreateDirectoryDialog
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import io.github.stratayun.paranoic.ui.state.UiLanguage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** Real Android rendering/input, isolated from Core, accounts and user folders. */
@RunWith(AndroidJUnit4::class)
class CreateDirectoryDialogTest {
    @get:Rule val compose = createComposeRule()

    @Test fun invalidNameCannotSubmitAndValidNameIsPassedUnchanged() {
        var submitted: String? = null
        compose.setContent {
            CompositionLocalProvider(LocalUiLanguage provides UiLanguage.English) {
                MaterialTheme { CreateDirectoryDialog(onDismiss = {}, onCreate = { submitted = it }) }
            }
        }
        compose.onNodeWithText("Create").assertIsNotEnabled()
        compose.onNodeWithText("Name").performTextReplacement("../outside")
        compose.onNodeWithText("Create").assertIsNotEnabled()
        compose.runOnIdle { assertNull(submitted) }
        val name = "Тестовая папка"
        compose.onNodeWithText("Name").performTextReplacement(name)
        compose.onNodeWithText("Create").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(name, submitted) }
    }

    @Test fun russianCancelDismissesWithoutSubmitting() {
        var submitted: String? = null
        var dismissed = false
        compose.setContent {
            CompositionLocalProvider(LocalUiLanguage provides UiLanguage.Russian) {
                MaterialTheme {
                    CreateDirectoryDialog(onDismiss = { dismissed = true }, onCreate = { submitted = it })
                }
            }
        }
        compose.onNodeWithText("Имя").performTextReplacement("Не создавать")
        compose.onNodeWithText("Отмена").performClick()
        compose.runOnIdle {
            assertEquals(true, dismissed)
            assertNull(submitted)
        }
    }
}
