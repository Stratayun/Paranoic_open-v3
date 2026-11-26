package io.github.stratayun.paranoic.platform

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.PlatformTextInputMethodRequest
import androidx.compose.ui.platform.InterceptPlatformTextInput

/** Audit 26.11: the keyboard must not learn words from Safe notes or vault text. */
@OptIn(ExperimentalComposeUiApi::class)
internal val incognitoKeyboard: @Composable (@Composable () -> Unit) -> Unit = { content ->
    InterceptPlatformTextInput(interceptor = { request, next ->
        next.startInputMethod(object : PlatformTextInputMethodRequest {
            override fun createInputConnection(outAttributes: EditorInfo): InputConnection {
                val connection = request.createInputConnection(outAttributes)
                outAttributes.imeOptions = outAttributes.imeOptions or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
                return connection
            }
        })
    }, content = content)
}

/**
 * Audit 26.11: text copied inside Paranoic is marked sensitive (Android 13+ hides it from
 * the clipboard preview) and cleared after two minutes unless something else was copied
 * since, as password managers do. Copies made while another app is in front are left alone.
 */
internal object AndroidClipboardHygiene {
    private const val CLEAR_AFTER_MS = 120_000L
    private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
    private val main = Handler(Looper.getMainLooper())
    private var generation = 0L
    private var rewriting = false
    @Volatile var foreground = false

    fun install(context: Context) {
        val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
        clipboard.addPrimaryClipChangedListener {
            generation++
            if (rewriting || !foreground) return@addPrimaryClipChangedListener
            val clip = try { clipboard.primaryClip } catch (_: Exception) { null } ?: return@addPrimaryClipChangedListener
            if (Build.VERSION.SDK_INT >= 33 && clip.description.extras?.getBoolean(EXTRA_IS_SENSITIVE) != true) {
                val marked = ClipData(clip.description, clip.getItemAt(0)).apply {
                    for (i in 1 until clip.itemCount) addItem(clip.getItemAt(i))
                    description.extras = PersistableBundle().apply { putBoolean(EXTRA_IS_SENSITIVE, true) }
                }
                rewriting = true
                try { clipboard.setPrimaryClip(marked) } catch (_: Exception) {} finally { rewriting = false }
            }
            val mine = generation
            main.postDelayed({
                if (generation == mine) try {
                    if (Build.VERSION.SDK_INT >= 28) clipboard.clearPrimaryClip() else clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
                } catch (_: Exception) {}
            }, CLEAR_AFTER_MS)
        }
    }
}
