package io.github.stratayun.paranoic.platform

import android.os.Bundle
import android.content.Intent
import android.view.WindowManager
import androidx.activity.compose.setContent
import androidx.activity.compose.BackHandler
import androidx.fragment.app.FragmentActivity
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import io.github.stratayun.paranoic.bridge.CoreGateway
import io.github.stratayun.paranoic.bridge.DisconnectedCoreGateway
import io.github.stratayun.paranoic.ui.ParanoicApp
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

class MainActivity : FragmentActivity() {
    private val incomingScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val incoming by lazy { AndroidIncomingDocuments(contentResolver, incomingScope) }
    private val preferences by lazy { AndroidAppearanceStore(applicationContext) }
    private val vaultCatalogStore by lazy { androidVaultCatalogStore(applicationContext) }
    private val placeCatalogStore by lazy { androidPlaceCatalogStore(applicationContext) }
    private val screenshotPolicy by lazy { AndroidScreenshotPolicy(applicationContext) }
    private fun applyScreenshotPolicy(allowed: Boolean) {
        if (allowed) window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        else window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) incoming.accept(intent)
        setIntent(Intent(this, MainActivity::class.java).setAction(Intent.ACTION_MAIN))
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        // Audit 26.11 (Android 14+): screen readers that are real accessibility tools (TalkBack)
        // keep working; other accessibility services can no longer read the app's text or input.
        if (android.os.Build.VERSION.SDK_INT >= 34)
            window.decorView.setAccessibilityDataSensitive(android.view.View.ACCESSIBILITY_DATA_SENSITIVE_YES)
        val app = application as ParanoicApplication
        setContent {
            val allowCapture = screenshotPolicy.allowed
            SideEffect { applyScreenshotPolicy(allowCapture) }
            var language by remember { mutableStateOf(preferences.read().language) }
            val context = this@MainActivity
            CompositionLocalProvider(LocalUiLanguage provides language,
                io.github.stratayun.paranoic.ui.state.LocalVolumeUsage provides { label -> AndroidStorageAccess.usage(context, label) }) {
            val authenticate = rememberAndroidAuthentication(this)
            val notesBiometric = rememberAndroidNotesBiometric(this, app.notesQuickKey)
            var attempt by remember { mutableIntStateOf(0) }
            var withoutCore by remember { mutableStateOf(false) }
            val startup by produceState<Startup>(Startup.Loading, app, attempt) {
                value = Startup.Loading
                value = try { Startup.Ready(app.runtime().await()) }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (error: Exception) { Startup.Failed(startupFault(error)) }
                catch (error: LinkageError) { Startup.Failed(startupFault(error)) }
            }
            val core: CoreGateway? = when (val s = startup) {
                is Startup.Ready -> s.core
                is Startup.Failed -> if (withoutCore) DisconnectedCoreGateway else null
                Startup.Loading -> null
            }
            // Saved language also for the screens outside ParanoicApp (system auth
            // gate, startup failure); ParanoicApp provides the live value inside.
            MaterialTheme {
                AndroidAppGate(this, app.security, authenticate) {
                    val ready = core
                    val failed = startup as? Startup.Failed
                    if (ready == null && failed != null) StartupFailure(failed.fault,
                        onRetry = { withoutCore = false; attempt++ }, onContinue = { withoutCore = true })
                    else if (ready == null) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                    else AndroidStorageAccessGate(ready, preferences.read()) { ParanoicApp(
                        core = ready, store = preferences, vaultCatalogStore = vaultCatalogStore, placeCatalogStore = placeCatalogStore,
                        diagnosticEnvironment = app.diagnostics,
                        onLanguageChange = { language = it },
                        notesBiometric = notesBiometric,
                        incomingDocuments = incoming,
                        notesImage = { bytes, modifier -> AndroidNoteImage(bytes, modifier) },
                        folderPicker = { onResult -> rememberAndroidVolumePicker(onResult) },
                        pngImage = { bytes, modifier -> AndroidPng(bytes, modifier) },
                        mediaPlayer = { gateway, location, info, modifier -> AndroidMediaPlayer(gateway, location, info, modifier) },
                        securitySettings = {
                            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                AndroidSecuritySettings(app.security, authenticate)
                                HorizontalDivider()
                                AndroidScreenshotSettings(screenshotPolicy, ::applyScreenshotPolicy)
                            }
                        },
                        backHandler = { enabled, action -> BackHandler(enabled, onBack = action) },
                        sessionLifecycle = { session ->
                            DisposableEffect(session) {
                                val observer = LifecycleEventObserver { _, event ->
                                    when (event) {
                                        Lifecycle.Event.ON_START -> session.resume()
                                        Lifecycle.Event.ON_STOP -> session.pause()
                                        else -> Unit
                                    }
                                }
                                lifecycle.addObserver(observer)
                                onDispose { lifecycle.removeObserver(observer); session.pause() }
                            }
                        },
                    ) }
                }
            } }
        }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        incoming.accept(intent)
        setIntent(Intent(this, MainActivity::class.java).setAction(Intent.ACTION_MAIN))
    }
    override fun onDestroy() {
        incoming.destroy()
        incomingScope.cancel()
        super.onDestroy()
    }
}
