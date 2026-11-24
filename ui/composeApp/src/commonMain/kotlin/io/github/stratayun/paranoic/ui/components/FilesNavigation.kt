package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

/** v1.5 breadcrumb presentation with Core identities instead of Android refs. */
@Composable
fun FilesNavigation(trail: List<FolderNode>, onBreadcrumb: (Int) -> Unit) {
    LazyRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        itemsIndexed(trail) { index, folder ->
            TextButton(enabled = index != trail.lastIndex, onClick = { onBreadcrumb(index) }) {
                Text(displayName(folder.name), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.widthIn(max = 200.dp))
            }
        }
    }
}

