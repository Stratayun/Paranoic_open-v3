package io.github.stratayun.paranoic.platform

import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.util.AtomicFile
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.biometric.BiometricManager.Authenticators
import androidx.biometric.BiometricPrompt
import androidx.fragment.app.FragmentActivity
import androidx.core.content.ContextCompat
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.announceChanges
import io.github.stratayun.paranoic.ui.state.sectionHeading
import io.github.stratayun.paranoic.ui.components.LabeledSwitch
import java.io.File

class AndroidSecurityState(context: Context) {
    private val setting = AtomicFile(File(context.noBackupFilesDir, "app-auth-v1"))
    val available = context.getSystemService(KeyguardManager::class.java).isDeviceSecure
    var enabled by mutableStateOf(if (setting.baseFile.exists() || File(setting.baseFile.path + ".bak").exists())
        setting.openRead().use { it.read() != 0 } else false)
        private set
    fun set(value: Boolean) {
        val output = setting.startWrite()
        try { output.write(if (value) 1 else 0); setting.finishWrite(output); enabled = value }
        catch (error: Exception) { setting.failWrite(output); throw error }
    }
}
@Composable
fun rememberAndroidAuthentication(activity: FragmentActivity): ((Boolean) -> Unit) -> Unit {
    var pending by remember { mutableStateOf<((Boolean) -> Unit)?>(null) }
    val subtitle = tr("Системный вход", "System authentication")
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        val callback = pending; pending = null; callback?.invoke(it.resultCode == android.app.Activity.RESULT_OK)
    }
    // Register this flow's callback when starting it; notes use their own prompt on the same Activity.
    fun newPrompt() = BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val callback = pending; pending = null; callback?.invoke(true)
            }
            override fun onAuthenticationError(code: Int, message: CharSequence) {
                val callback = pending; pending = null; callback?.invoke(false)
            }
        })
    return { callback ->
        if (pending == null) {
            pending = callback
            try {
                if (Build.VERSION.SDK_INT >= 30) newPrompt().authenticate(BiometricPrompt.PromptInfo.Builder()
                    .setTitle("Paranoic").setSubtitle(subtitle)
                    .setAllowedAuthenticators(Authenticators.BIOMETRIC_STRONG or Authenticators.DEVICE_CREDENTIAL).build())
                else {
                    val intent = activity.getSystemService(KeyguardManager::class.java)
                        .createConfirmDeviceCredentialIntent("Paranoic", subtitle)
                    if (intent == null) { pending = null; callback(false) } else launcher.launch(intent)
                }
            } catch (_: Exception) { pending = null; callback(false) }
        }
    }
}
/** Modal system-auth gate. Underlying composition/picker remains registered.
 * Device authentication protects entry to UI; it does not encrypt ordinary files.
 * A result unlocks only the attempt it belongs to, and only while the app is
 * on screen (У-42): leaving the app without a running attempt starts a new
 * generation, so an earlier attempt's late success is ignored; a success that
 * arrives while the app is in the background is ignored too. An attempt that
 * is running when the app stops keeps its generation, because on API 26–29
 * (and for the device-credential fallback) the system auth screen itself is a
 * separate activity that stops this one.
 */
@Composable
fun AndroidAppGate(activity: FragmentActivity, state: AndroidSecurityState,
    authenticate: ((Boolean) -> Unit) -> Unit, content: @Composable () -> Unit) {
    var unlocked by remember { mutableStateOf(!state.enabled) }
    var busy by remember { mutableStateOf(false) }
    var generation by remember { mutableStateOf(0) }
    DisposableEffect(activity, state) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP && state.enabled) {
                unlocked = false
                if (!busy) generation++
            }
        }
        activity.lifecycle.addObserver(observer)
        onDispose { activity.lifecycle.removeObserver(observer) }
    }
    content()
    if (state.enabled && !unlocked) Dialog(onDismissRequest = {},
        properties = DialogProperties(dismissOnBackPress = false, dismissOnClickOutside = false,
            usePlatformDefaultWidth = false, securePolicy = SecureFlagPolicy.SecureOn)) {
        Surface(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize().padding(24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Paranoic", Modifier.sectionHeading())
                Button(enabled = !busy, onClick = {
                    busy = true
                    val attempt = generation
                    authenticate { success ->
                        busy = false
                        val onScreen = activity.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
                        if (success && attempt == generation && onScreen) unlocked = true
                    }
                }) { Text(tr("Разблокировать", "Unlock")) }
            }
        }
    }
}
@Composable
fun AndroidSecuritySettings(state: AndroidSecurityState, authenticate: ((Boolean) -> Unit) -> Unit) {
    var failed by remember { mutableStateOf(false) }
    Column {
        LabeledSwitch(tr("Системный вход в приложение", "System authentication for app entry"),
            checked = state.enabled, enabled = state.available, onChange = { enabled ->
            authenticate { success ->
                if (success) try { state.set(enabled); failed = false } catch (_: Exception) { failed = true }
                else failed = true
            }
        })
        if (!state.available) Text(tr("Сначала настройте блокировку устройства.", "Set up device screen lock first."))
        Text(tr("Это защита входа в UI, не пароль Private Vault.", "This protects UI entry and is not a Private Vault password."))
        if (failed) Text(tr("Настройка не изменена.", "Setting was not changed."), Modifier.announceChanges(), color = MaterialTheme.colorScheme.error)
    }
}
