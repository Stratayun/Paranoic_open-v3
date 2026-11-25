package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

/**
 * Location header (reference: rounded card with the current folder and its
 * path). "Up" returns one level; each path segment is tappable.
 */
@Composable
fun FilesNavigation(trail: List<FolderNode>, onBreadcrumb: (Int) -> Unit) {
    if (trail.isEmpty()) return
    val scheme = MaterialTheme.colorScheme
    val crumbs = rememberLazyListState()
    LaunchedEffect(trail.size) { crumbs.scrollToItem(trail.lastIndex) }
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = scheme.surfaceContainer) {
        Row(Modifier.padding(start = 4.dp, end = 12.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { onBreadcrumb(trail.lastIndex - 1) }, enabled = trail.size > 1,
                modifier = Modifier.actionDescription(tr("На уровень выше", "Up one level"))) { AppIcon(AppSymbol.Up) }
            Column(Modifier.weight(1f)) {
                Text(displayName(trail.last().name), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (trail.size > 1) LazyRow(state = crumbs, verticalAlignment = Alignment.CenterVertically) {
                    itemsIndexed(trail) { index, folder -> Row(verticalAlignment = Alignment.CenterVertically) {
                        if (index > 0) Text(" › ", color = scheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
                        Text(displayName(folder.name), maxLines = 1, overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = if (index == trail.lastIndex) scheme.primary else scheme.onSurfaceVariant,
                            modifier = Modifier.widthIn(max = 160.dp).clip(MaterialTheme.shapes.small)
                                .clickable(enabled = index != trail.lastIndex) { onBreadcrumb(index) }.padding(vertical = 4.dp, horizontal = 2.dp))
                    } }
                }
            }
        }
    }
}
