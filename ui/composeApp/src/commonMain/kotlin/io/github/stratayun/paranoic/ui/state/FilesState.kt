package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

enum class FileOrder { NameAscending, NameDescending, SizeAscending, SizeDescending }
data class FileListOptions(
    val query: String = "",
    val order: FileOrder = FileOrder.NameAscending,
    val showHidden: Boolean = false,
)
data class FolderNode(val location: Location, val name: String)
data class FilesState(
    val spaces: List<Space> = emptyList(),
    val trail: List<FolderNode> = emptyList(),
    val entries: List<FileItem> = emptyList(),
    /** Entries Core omitted (non-portable or duplicate names). */
    val hidden: Long = 0,
    val selected: Set<Location> = emptySet(),
    val loading: Boolean = false,
    val loaded: Boolean = false,
    val fault: CoreFault? = null,
) {
    val folder: Location? get() = trail.lastOrNull()?.location
    val writable: Boolean get() = spaces.any { it.root.storageId == folder?.storageId && it.writable }
}
/** Only a projection of loaded Core metadata; no filesystem or permission logic. */
fun visibleFiles(entries: List<FileItem>, options: FileListOptions): List<FileItem> {
    val filtered = entries.filter {
        (options.showHidden || !it.name.startsWith(".")) && it.name.contains(options.query, ignoreCase = true)
    }
    val ordered = when (options.order) {
        FileOrder.NameAscending -> filtered.sortedBy { it.name.lowercase() }
        FileOrder.NameDescending -> filtered.sortedByDescending { it.name.lowercase() }
        FileOrder.SizeAscending -> filtered.sortedWith(compareBy<FileItem> { it.bytes }.thenBy { it.name.lowercase() })
        FileOrder.SizeDescending -> filtered.sortedWith(compareByDescending<FileItem> { it.bytes }.thenBy { it.name.lowercase() })
    }
    return ordered.sortedBy { if (it.kind == EntryKind.Directory) 0 else 1 }
}


/** Paranoic's own app-data folder (Core workspace). Not shown as a user place: the encrypted personal folder replaces it. */
fun io.github.stratayun.paranoic.bridge.Space.isAppPrivate() = localContainer && !detachable && !cloud
fun List<io.github.stratayun.paranoic.bridge.Space>.userPlaces() = filterNot { it.isAppPrivate() }
