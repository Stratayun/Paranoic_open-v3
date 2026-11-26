package io.github.stratayun.paranoic.ui.components

import androidx.compose.runtime.Composable

/**
 * Text that must not leave the app through the keyboard (audit 26.11): Safe notes and text
 * files inside vaults. Android installs a wrapper that asks the keyboard not to learn from
 * the fields inside (IME_FLAG_NO_PERSONALIZED_LEARNING, "incognito"); elsewhere it is a no-op.
 */
object SecretTextInput {
    var wrapper: (@Composable (@Composable () -> Unit) -> Unit)? = null
    /** Android 14+: marks the current window (a dialog is a window of its own) as accessibility-sensitive. */
    var sensitiveWindow: (@Composable () -> Unit)? = null
}

/** Place inside dialogs that show or take secrets: only real accessibility tools may read them. */
@Composable
fun SensitiveWindow() { SecretTextInput.sensitiveWindow?.invoke() }

@Composable
fun SecretInput(content: @Composable () -> Unit) {
    SensitiveWindow()
    val wrapper = SecretTextInput.wrapper
    if (wrapper != null) wrapper(content) else content()
}
