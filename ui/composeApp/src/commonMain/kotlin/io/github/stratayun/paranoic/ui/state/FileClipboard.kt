package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.stratayun.paranoic.bridge.FileItem
import io.github.stratayun.paranoic.bridge.Location
import io.github.stratayun.paranoic.bridge.NameConflict

/** In-memory "Copy / Cut → Paste" buffer of the file screens. Holds Core locations only, never file contents. */
class FileClipboard {
    var items by mutableStateOf<List<FileItem>>(emptyList()); private set
    var kind by mutableStateOf(FileBatchKind.Copy); private set
    val isEmpty get() = items.isEmpty()

    fun put(sources: List<FileItem>, cut: Boolean) {
        items = sources.take(MAX_FILE_SELECTION)
        kind = if (cut) FileBatchKind.Move else FileBatchKind.Copy
    }
    fun clear() { items = emptyList() }

    /** Null when pasting here is impossible (same folder for a move, mixed parents, empty buffer). */
    fun plan(parent: Location): FileBatchPlan? = runCatching {
        FileBatchPlan(kind, items, parent, if (kind == FileBatchKind.Copy) NameConflict.KeepBoth else NameConflict.Fail)
    }.getOrNull()
}
