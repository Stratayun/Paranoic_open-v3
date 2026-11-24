package io.github.stratayun.paranoic.platform

import android.os.Build
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.compose.runtime.*
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import io.github.stratayun.paranoic.bridge.NotesBiometric
import io.github.stratayun.paranoic.ui.state.tr
import kotlinx.coroutines.*
import kotlin.coroutines.resume

private class AndroidNotesBiometric(private val activity: FragmentActivity, private val key: AndroidNotesQuickKey,
    private val scope: CoroutineScope) : NotesBiometric {
    var labels = NotesPromptLabels("Paranoic", "Quick notes unlock", "Password")
    override val available get() = Build.VERSION.SDK_INT >= 30 && BiometricManager.from(activity)
        .canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS
    private var pending: CancellableContinuation<Boolean>? = null
    private var prompt: BiometricPrompt? = null
    override suspend fun authenticate(enabling: Boolean): Boolean {
        if (!available || pending != null) return false
        val operation = try { withContext(Dispatchers.IO) { key.prepare(enabling) } }
            catch (cancelled: CancellationException) { key.revoke(); throw cancelled }
            catch (_: Exception) { return false }
        return suspendCancellableCoroutine { continuation ->
            pending = continuation
            val activePrompt = BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    if (pending !== continuation) return
                    pending = null; prompt = null
                    scope.launch {
                        val success = withContext(Dispatchers.IO) { result.cryptoObject?.cipher?.let { key.authorize(operation, it) } == true }
                        if (continuation.isActive) continuation.resume(success) else key.revoke()
                    }
                }
                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    if (pending !== continuation) return
                    pending = null; prompt = null; key.revoke()
                    if (continuation.isActive) continuation.resume(false)
                }
            })
            prompt = activePrompt
            continuation.invokeOnCancellation { activity.runOnUiThread { cancel() } }
            val text = labels
            try { activePrompt.authenticate(BiometricPrompt.PromptInfo.Builder().setTitle(text.title)
                .setSubtitle(text.subtitle)
                .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                .setNegativeButtonText(text.password).build(), BiometricPrompt.CryptoObject(operation.cipher)) }
            catch (_: Exception) { cancel() }
        }
    }
    override fun cancel() {
        val continuation = pending; pending = null
        val activePrompt = prompt; prompt = null
        activePrompt?.cancelAuthentication(); key.revoke()
        if (continuation?.isActive == true) continuation.resume(false)
    }
}
private data class NotesPromptLabels(val title: String, val subtitle: String, val password: String)
@Composable
fun rememberAndroidNotesBiometric(activity: FragmentActivity, key: AndroidNotesQuickKey): NotesBiometric {
    val scope = rememberCoroutineScope()
    val authentication = remember(activity, key, scope) { AndroidNotesBiometric(activity, key, scope) }
    val labels = NotesPromptLabels(tr("Paranoic · Заметки", "Paranoic · Notes"),
        tr("Быстрый вход в заметки", "Quick notes unlock"), tr("Пароль", "Password"))
    SideEffect { authentication.labels = labels }
    DisposableEffect(activity, authentication) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) authentication.cancel() }
        activity.lifecycle.addObserver(observer)
        onDispose { activity.lifecycle.removeObserver(observer); authentication.cancel() }
    }
    return authentication
}
