package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.ImageComposeScene
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.DrawerFooter
import io.github.stratayun.paranoic.ui.components.DrawerHeader
import io.github.stratayun.paranoic.ui.components.SpacesPanel
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.theme.ParanoicTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.jetbrains.skia.EncodedImageFormat
import java.io.File
import kotlin.test.Test

/**
 * Developer preview, not a check: renders the phone drawer to a PNG when
 * PARANOIC_PREVIEW_OUT is set (otherwise does nothing).
 */
class DrawerPreviewShot {
    @Test fun drawer() {
        val out = System.getenv("PARANOIC_PREVIEW_OUT") ?: return
        val places = runBlocking { PlaceCatalog(MemoryPlaceCatalogStore(), kotlinx.coroutines.CoroutineScope(Dispatchers.Unconfined)) }
        fun space(id: String, name: String, detachable: Boolean = true, cloud: Boolean = false, tree: Boolean = false, local: Boolean = false) =
            Space(id, name, Location(id, ""), true, detachable = detachable, androidTree = tree, localContainer = local, cloud = cloud)
        val spaces = listOf(space("app", "App", detachable = false, local = true), space("dev", "Внутренняя память"),
            space("sd", "SD-карта"), space("cloud", "Nextcloud", cloud = true))
        val scene = ImageComposeScene(1080, 2700, Density(2.625f)) {
            ParanoicTheme(AppearanceSettings()) {
                CompositionLocalProvider(LocalVolumeUsage provides { VolumeUsage(41L shl 30, 128L shl 30) }) {
                    Surface(color = MaterialTheme.colorScheme.surfaceContainerLow) {
                        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                            DrawerHeader {}
                            SpacesPanel(spaces, places, connected = true, busy = false, onConnect = {}, onOpen = {}, onDetach = {},
                                onConnectServer = { _, _, _ -> }, onOpenPath = { _, _ -> }, onOpenCategory = { _, _ -> }, onOpenTrash = {},
                                trashFilled = setOf("dev"))
                            DrawerFooter(enabled = true, runningTasks = 0, onSafe = {}, onVaults = {}, onSync = {}, onDiagnostics = {},
                                onTasks = {}, onSettings = {}, showFiles = false)
                            Spacer(Modifier.height(24.dp))
                        }
                    }
                }
            }
        }
        val image = scene.render()
        File(out).writeBytes(image.encodeToData(EncodedImageFormat.PNG)!!.bytes)
        scene.close()
    }
}
