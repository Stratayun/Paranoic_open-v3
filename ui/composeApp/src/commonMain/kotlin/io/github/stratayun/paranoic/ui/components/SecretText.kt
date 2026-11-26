package io.github.stratayun.paranoic.ui.components

import androidx.compose.runtime.Composable

/**
 * Text that must not leave the app through the keyboard (audit 26.11): Safe notes and text
 * files inside vaults. Android installs a wrapper that asks the keyboard not to learn from
 * the fields inside (IME_FLAG_NO_PERSONALIZED_LEARNING, "incognito"); elsewhere it is a no-op.
 */
object SecretTextInput {
    var wrapper: (@Composable (@Composable () -> Unit) -> Unit)? = null
}

@Composable
fun SecretInput(content: @Composable () -> Unit) {
    val wrapper = SecretTextInput.wrapper
    if (wrapper != null) wrapper(content) else content()
}
