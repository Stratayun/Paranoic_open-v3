package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.displayName
import io.github.stratayun.paranoic.ui.state.sectionHeading

/** One tile of the file action sheet. A disabled tile stays visible so the option is discoverable. */
data class SheetAction(val label: String, val symbol: AppSymbol, val enabled: Boolean = true, val danger: Boolean = false, val run: () -> Unit)
data class SheetGroup(val title: String, val actions: List<SheetAction>)

/** Long-press sheet (reference: Strata): file header, then groups of 3-column tiles. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FileActionSheet(title: String, subtitle: String, symbol: AppSymbol, groups: List<SheetGroup>, onDismiss: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = scheme.surfaceContainerLow) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                ShapedIcon(symbol, ExpressiveShapes.Cookie9, scheme.primaryContainer, scheme.onPrimaryContainer)
                Column(Modifier.weight(1f)) {
                    Text(displayName(title), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                }
            }
            groups.filter { it.actions.isNotEmpty() }.forEach { group ->
                Text(group.title, Modifier.sectionHeading().padding(start = 4.dp, top = 20.dp, bottom = 8.dp),
                    style = MaterialTheme.typography.titleSmall, color = scheme.primary)
                group.actions.chunked(3).forEach { row ->
                    Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        row.forEach { action -> Tile(action, Modifier.weight(1f)) }
                        repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
                    }
                }
            }
        }
    }
}

@Composable
private fun Tile(action: SheetAction, modifier: Modifier) {
    val scheme = MaterialTheme.colorScheme
    val tint = when { !action.enabled -> scheme.onSurface.copy(alpha = 0.38f); action.danger -> scheme.error; else -> scheme.onSurface }
    Surface(modifier.heightIn(min = 96.dp), shape = MaterialTheme.shapes.large, color = scheme.surfaceContainerHigh) {
        Column(Modifier.clickable(enabled = action.enabled, role = Role.Button, onClick = action.run).padding(12.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically)) {
            CompositionLocalProviderTint(tint) { AppIcon(action.symbol) }
            Text(action.label, style = MaterialTheme.typography.labelLarge, color = tint, textAlign = TextAlign.Center, maxLines = 2)
        }
    }
}

@Composable
private fun CompositionLocalProviderTint(color: Color, content: @Composable () -> Unit) =
    androidx.compose.runtime.CompositionLocalProvider(LocalContentColor provides color, content = content)
