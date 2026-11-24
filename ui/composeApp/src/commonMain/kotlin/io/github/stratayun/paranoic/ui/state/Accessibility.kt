package io.github.stratayun.paranoic.ui.state

import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.foundation.border
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.*
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** UI semantics only; never announce document contents, passwords or recovery keys as status. */
fun Modifier.sectionHeading(): Modifier = semantics { heading() }
fun Modifier.announceChanges(): Modifier = semantics { liveRegion = LiveRegionMode.Polite }
fun Modifier.actionDescription(label: String): Modifier = semantics { contentDescription = label }
fun Modifier.fieldError(message: String, invalid: Boolean): Modifier = semantics { if (invalid) error(message) }

/** Use before clickable/selectable so it observes that control's focus target. */
@Composable
fun Modifier.keyboardFocusOutline(): Modifier {
    var focused by remember { mutableStateOf(false) }
    val color = MaterialTheme.colorScheme.primary
    return onFocusChanged { focused = it.hasFocus }.border(2.dp, if (focused) color else Color.Transparent)
}
