package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.ImageComposeScene
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Density
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.AppIcon
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.navigation.Destination
import io.github.stratayun.paranoic.ui.navigation.bottomTabs
import io.github.stratayun.paranoic.ui.screens.home.HomeScreen
import io.github.stratayun.paranoic.ui.screens.home.ModulesScreen
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.theme.ParanoicTheme
import kotlinx.coroutines.Dispatchers
import org.jetbrains.skia.EncodedImageFormat
import java.io.File
import kotlin.test.Test

/** Developer preview, not a check: renders Home and Modules to PNGs when PARANOIC_PREVIEW_OUT is set. */
class HomePreviewShot {
    @OptIn(ExperimentalMaterial3Api::class)
    @Test fun home() {
        val out = System.getenv("PARANOIC_PREVIEW_OUT") ?: return
        val places = PlaceCatalog(MemoryPlaceCatalogStore(), kotlinx.coroutines.CoroutineScope(Dispatchers.Unconfined))
        places.pin(PlaceBookmark("dev", "Documents/Projects", "Проекты"))
        places.pin(PlaceBookmark("usb", "Backup", "Backup"))
        fun space(id: String, name: String, detachable: Boolean = true, cloud: Boolean = false, local: Boolean = false) =
            Space(id, name, Location(id, ""), true, detachable = detachable, localContainer = local, cloud = cloud)
        val spaces = listOf(space("app", "App", detachable = false, local = true), space("dev", "Внутренняя память"),
            space("usb", "USB-накопитель"), space("cloud", "Мой сервер", cloud = true))
        for ((name, tab) in listOf("home" to Destination.Home, "modules" to Destination.Modules, "settings" to Destination.Settings)) {
            val scene = ImageComposeScene(1080, 2400, Density(2.625f)) {
                ParanoicTheme(AppearanceSettings()) {
                    CompositionLocalProvider(LocalVolumeUsage provides { label -> if (label.startsWith("USB")) VolumeUsage(12L shl 30, 64L shl 30) else VolumeUsage(118L shl 30, 256L shl 30) }) {
                        Scaffold(topBar = { TopAppBar(title = { Text(if (tab == Destination.Home) "Paranoic" else tab.label()) },
                            actions = { IconButton(onClick = {}) { AppIcon(AppSymbol.Search) }; IconButton(onClick = {}) { AppIcon(AppSymbol.Tasks) } }) },
                            bottomBar = { NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                                bottomTabs.forEach { t -> NavigationBarItem(selected = t == tab, onClick = {}, label = { Text(if (t == Destination.Sync) "Синхронизация" else t.label(), style = MaterialTheme.typography.labelSmall, maxLines = 1, softWrap = false) },
                                    icon = { AppIcon(when (t) { Destination.Home -> AppSymbol.Home; Destination.Files -> AppSymbol.Folder
                                        Destination.Modules -> AppSymbol.ViewGrid; Destination.Sync -> AppSymbol.Sync; else -> AppSymbol.Settings }) }) }
                            } }) { insets ->
                            Box(Modifier.padding(insets)) {
                                when (tab) {
                                    Destination.Home -> HomeScreen(spaces, places, {}, {}, {}, {}, {}, { _, _ -> }, {}, {})
                                    Destination.Settings -> io.github.stratayun.paranoic.ui.screens.settings.AppearanceScreen(AppearanceSettings(), false, null, {})
                                    else -> ModulesScreen({}, {}, {}, {}, {}, {}, {}, {}, {}, {})
                                }
                            }
                        }
                    }
                }
            }
            for (t in 0..16) scene.render(t * 100_000_000L)
            File(out, "$name.png").writeBytes(scene.render(1_600_000_000L).encodeToData(EncodedImageFormat.PNG)!!.bytes)
            scene.close()
        }
    }
}
