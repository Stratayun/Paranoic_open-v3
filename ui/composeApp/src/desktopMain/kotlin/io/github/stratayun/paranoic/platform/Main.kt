package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.ParanoicApp
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.nio.file.Paths
import com.sun.jna.Native

/**
 * A failure before the window exists (missing runtime module, native library)
 * would otherwise end the app silently: write it next to the app data and show it.
 */
fun main() {
    DesktopPrivateTemp.install(Paths.get(System.getenv("LOCALAPPDATA") ?: System.getProperty("user.home"), "Paranoic").toAbsolutePath())
    try { runApp() } catch (error: Throwable) {
        val text = buildString {
            appendLine("Paranoic could not start (${System.getProperty("os.name")}, Java ${System.getProperty("java.version")}).")
            appendLine(error.stackTraceToString().take(6000))
        }
        // Next to Paranoic.exe first (easy to find), then the user's app data.
        val besideExe = System.getProperty("jpackage.app-path")?.let { Paths.get(it).toAbsolutePath().parent?.resolve("startup-error.txt") }
        val inAppData = Paths.get(System.getenv("LOCALAPPDATA") ?: System.getProperty("user.home"), "Paranoic", "startup-error.txt")
        val file = listOfNotNull(besideExe, inAppData).firstNotNullOfOrNull { target ->
            try { java.nio.file.Files.createDirectories(target.parent); java.nio.file.Files.writeString(target, text); target } catch (_: Exception) { null }
        }
        try {
            javax.swing.JOptionPane.showMessageDialog(null, (text.lineSequence().take(12).joinToString("\n")) +
                (file?.let { "\n\nПолный текст / full text: $it" } ?: ""), "Paranoic", javax.swing.JOptionPane.ERROR_MESSAGE)
        } catch (_: Throwable) {}
        kotlin.system.exitProcess(1)
    }
}

private fun runApp() = application {
    val scope = rememberCoroutineScope()
    var startup by remember { mutableStateOf<Startup>(Startup.Loading) }
    var attempt by remember { mutableIntStateOf(0) }
    var withoutCore by remember { mutableStateOf(false) }
    var closing by remember { mutableStateOf(false) }
    val root = remember {
        Paths.get(System.getenv("LOCALAPPDATA") ?: System.getProperty("user.home"), "Paranoic").toAbsolutePath()
    }
    val vaultCatalogStore = remember(root) { desktopVaultCatalogStore(root) }
    val placeCatalogStore = remember(root) { desktopPlaceCatalogStore(root) }
    val diagnostics = remember(root) {
        JvmDiagnosticEnvironment(root.resolve("last-jvm-crash-v1.bin")) {
            io.github.stratayun.paranoic.ui.state.DiagnosticPlatformFacts(System.getProperty("os.name", "Unavailable"),
                System.getProperty("os.version", "Unavailable"), System.getProperty("os.arch", "Unavailable"),
                System.getProperty("jpackage.app-version") ?: System.getProperty("paranoic.app.version") ?: "Unavailable")
        }
    }
    LaunchedEffect(diagnostics) { diagnostics.install() }
    LaunchedEffect(root, attempt) {
        startup = Startup.Loading
        startup = try {
            val pdfium = System.getProperty("paranoic.pdfium") ?: try {
                Native.extractFromResourcePath("pdfium").absolutePath
            } catch (_: Exception) { root.resolve("native/pdfium.dll").toString() }
            val workerName = if (System.getProperty("os.name").startsWith("Windows")) "paranoic-pdf-worker.exe" else "paranoic-pdf-worker"
            // Only the app resources directory (read-only install dir, also set by `run`) or an
            // explicit developer override; never a user-writable or working-directory lookup.
            // Missing worker = PDF unavailable, the rest of the app still starts.
            val pdfWorker = System.getProperty("paranoic.pdfworker")
                ?: System.getProperty("compose.application.resources.dir")?.let { Paths.get(it, workerName).toString() }
                ?: ""
            Startup.Ready(NativeCoreGateway.open(root.resolve("state").toString(), root.resolve("files").toString(),
                pdfium, pdfWorker,
                NoAndroidDocuments, DesktopGrantStore(root), DesktopRecordSealer))
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) { Startup.Failed(startupFault(error)) }
        catch (error: LinkageError) { Startup.Failed(startupFault(error)) }
    }
    val core: CoreGateway? = when (val s = startup) {
        is Startup.Ready -> s.core
        is Startup.Failed -> if (withoutCore) DisconnectedCoreGateway else null
        Startup.Loading -> null
    }
    Window(onCloseRequest = {
        if (!closing) {
            closing = true
            scope.launch {
                try { (core as? NativeCoreGateway)?.shutdown() }
                finally { DesktopPrivateTemp.wipe(root.resolve("tmp")); exitApplication() }
            }
        }
    }, title = "Paranoic", state = rememberWindowState(width = 1100.dp, height = 760.dp)) {
        LaunchedEffect(Unit) { DesktopClipboardHygiene.start(this) { window.isFocused } }
        val ready = core
        val failed = startup as? Startup.Failed
        // Saved language also for the startup screens outside ParanoicApp.
        var language by remember { mutableStateOf(DesktopAppearanceStore.read().language) }
        CompositionLocalProvider(LocalUiLanguage provides language) {
        val attachmentPicker = rememberDesktopNotesAttachmentPicker(window)
        if (ready == null && failed != null) MaterialTheme {
            StartupFailure(failed.fault, onRetry = { withoutCore = false; attempt++ }, onContinue = { withoutCore = true })
        } else if (ready == null) MaterialTheme {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        } else ParanoicApp(
            core = ready, store = DesktopAppearanceStore, vaultCatalogStore = vaultCatalogStore, placeCatalogStore = placeCatalogStore,
            diagnosticEnvironment = diagnostics,
            onLanguageChange = { language = it },
            notesAttachmentPicker = attachmentPicker,
            notesImage = { bytes, modifier -> DesktopNoteImage(bytes, modifier) },
            folderPicker = { onResult -> rememberDesktopFolderPicker(onResult, window) },
            pngImage = { bytes, modifier -> DesktopPng(bytes, modifier) },
            mediaPlayer = { gateway, location, info, modifier -> DesktopMediaPlayer(gateway, location, info, modifier) },
            sessionLifecycle = { session -> DesktopSessionLifecycle(window, session) },
        )
        }
    }
}
