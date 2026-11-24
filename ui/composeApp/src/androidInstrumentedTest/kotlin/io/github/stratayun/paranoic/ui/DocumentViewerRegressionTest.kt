package io.github.stratayun.paranoic.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.screens.documents.FileViewer
import io.github.stratayun.paranoic.ui.state.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.test.assertEquals

/** Phone-sized, fake-Core cases; no native decoding or personal files. Not a full UI sweep. */
@RunWith(AndroidJUnit4::class)
class DocumentViewerRegressionTest {
    @get:Rule val compose = createComposeRule()

    private val version = TextFileVersion(8uL, 1uL, "0".repeat(64))
    private class TextCore(val content: String, val version: TextFileVersion) : CoreGateway by DisconnectedCoreGateway {
        override val connected = true
        var saved: String? = null
        var submittedVersion: TextFileVersion? = null
        val rows = content.lines().flatMap { it.chunked(16_384).ifEmpty { listOf("") } }
        override suspend fun textOpen(location: Location) = CoreResult.Value(TextDocument(content, TextEncoding.Utf8, version))
        override suspend fun textPagerOpen(location: Location) = CoreResult.Value(TextPager("fixture", rows.size.toLong(), TextEncoding.Utf8, content.length.toULong()))
        override suspend fun textPagerRead(handle: String, first: Long, count: Int) = CoreResult.Value(rows.drop(first.toInt()).take(count))
        override suspend fun textPagerClose(handle: String) = CoreResult.Value(Unit)
        override suspend fun textSave(location: Location, version: TextFileVersion, text: String): CoreResult<TextFileVersion> {
            submittedVersion = version; saved = text
            return CoreResult.Value(version.copy(modifiedMs = null))
        }
    }

    private fun showText(core: TextCore, largeFont: Boolean = false) {
        val file = FileItem(Location("test", "example.txt"), "example.txt", EntryKind.File, core.content.length.toULong())
        compose.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalUiLanguage provides UiLanguage.English,
                LocalDensity provides Density(density.density, if (largeFont) 2f else 1f)) {
                MaterialTheme {
                    Box(Modifier.requiredSize(320.dp, 280.dp)) {
                        FileViewer(core, file, {}, { _, _ -> }, { _, _, _, _ -> },
                            textParent = Location("test", ""), exitGuard = remember { DocumentExitGuard() })
                    }
                }
            }
        }
        compose.waitForIdle()
    }

    @Test fun saveAndDiscardStayVisibleWithMoreOpenInAShortWindow() {
        val core = TextCore("original", version)
        showText(core, largeFont = true)
        compose.onNodeWithText("Document text").performTextReplacement("edited")
        compose.onNodeWithText("More").performClick()
        compose.onNodeWithText("Save", substring = false).assertIsDisplayed().assertIsEnabled().performClick()
        compose.onNodeWithText("Don't save").assertIsDisplayed()
        compose.runOnIdle { assertEquals("edited", core.saved); assertEquals(version, core.submittedVersion) }
    }

    @Test fun textAboveOneMillionCharactersIsReadOnlyAndItsEndIsReachable() {
        val content = "BEGIN\n" + "x".repeat(1_100_000) + "\nEND-OF-DOCUMENT"
        val core = TextCore(content, version)
        showText(core)
        compose.onNodeWithText("Read only · up to 1 GiB.", substring = true).assertExists()
        compose.onNodeWithText("Document text").assertDoesNotExist()
        compose.onNodeWithText("Save", substring = false).assertDoesNotExist()
        compose.onNodeWithText("Don't save").assertDoesNotExist()
        compose.onNodeWithText("Back").assertIsDisplayed()
        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(core.rows.lastIndex)
        compose.onNodeWithText("END-OF-DOCUMENT", substring = true).assertExists()
        compose.runOnIdle { assertEquals(null, core.saved) }
    }

    @Test fun imageTapHidesOverlayAndControlsButtonRestoresIt() {
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override val connected = true
            override suspend fun inspect(location: Location) = CoreResult.Value(PreviewInfo(PreviewKind.Image, "image/png", 1uL))
            override suspend fun imagePng(location: Location, side: Int) = CoreResult.Value(byteArrayOf(1))
        }
        val file = FileItem(Location("test", "image.png"), "image.png", EntryKind.File, 1uL)
        compose.setContent {
            CompositionLocalProvider(LocalUiLanguage provides UiLanguage.English) {
                MaterialTheme {
                    Box(Modifier.requiredSize(320.dp, 400.dp)) {
                        FileViewer(core, file, {}, { _, modifier -> Box(modifier.testTag("image")) }, { _, _, _, _ -> })
                    }
                }
            }
        }
        compose.onNodeWithText("Back").assertExists()
        compose.onNodeWithTag("image").performTouchInput { click(center) }
        compose.onNodeWithText("Back").assertDoesNotExist()
        compose.onNodeWithText("Controls").performClick()
        compose.onNodeWithText("Back").assertExists()
    }
}
