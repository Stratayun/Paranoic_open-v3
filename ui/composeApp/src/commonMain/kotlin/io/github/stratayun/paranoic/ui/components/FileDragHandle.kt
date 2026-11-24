package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.key.*
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.FileItem
import io.github.stratayun.paranoic.ui.state.*

/** Internal pointer token only: no OS drag payload, clipboard, URI or temporary file. */
class FileDragHandler(val start: (FileItem, Offset) -> Unit, val move: (Offset) -> Unit,
    val end: () -> Unit, val cancel: () -> Unit, val copyToOtherPane: (FileItem) -> Unit)

@Composable
fun FileDragHandle(item: FileItem, handler: FileDragHandler) {
    var origin by remember { mutableStateOf(Offset.Zero) }
    val latest by rememberUpdatedState(handler)
    val label = tr("Копировать в другую панель: ", "Copy to other pane: ") + displayName(item.name)
    Box(Modifier.size(48.dp).onGloballyPositioned { origin = it.boundsInRoot().topLeft }
        .semantics { contentDescription = label; role = Role.Button; onClick(label) { latest.copyToOtherPane(item); true } }
        .keyboardFocusOutline().onKeyEvent {
            if (it.type == KeyEventType.KeyUp && (it.key == Key.Enter || it.key == Key.Spacebar)) {
                latest.copyToOtherPane(item); true
            } else false
        }.focusable().pointerInput(item.location) {
            detectDragGestures(onDragStart = { latest.start(item, origin + it) },
                onDragEnd = { latest.end() }, onDragCancel = { latest.cancel() },
                onDrag = { change, amount -> change.consume(); latest.move(amount) })
        }, contentAlignment = androidx.compose.ui.Alignment.Center) { Text("⇄") }
}
