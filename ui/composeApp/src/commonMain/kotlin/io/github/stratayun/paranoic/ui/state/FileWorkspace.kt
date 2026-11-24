package io.github.stratayun.paranoic.ui.state

const val MAX_FILE_TABS = 8
data class FileTab(val id: String, val files: FilesState = FilesState(), val options: FileListOptions = FileListOptions(),
    val view: FileView? = null)

/** Ephemeral navigation only. Lock discards all tabs, selections and paths. */
data class FileWorkspace(val tabs: List<FileTab> = listOf(FileTab("files-1")), val left: String = "files-1",
    val right: String? = null, val active: String = left, private val nextId: Int = 2) {
    init {
        require(tabs.size in 1..MAX_FILE_TABS && tabs.map { it.id }.distinct().size == tabs.size)
        require(tabs.any { it.id == left } && (right == null || tabs.any { it.id == right }))
        require(right != left && (active == left || active == right))
    }
    fun tab(id: String) = tabs.singleOrNull { it.id == id }
    val current get() = requireNotNull(tab(active))
    fun update(id: String, transform: (FileTab) -> FileTab): FileWorkspace = copy(tabs = tabs.map {
        if (it.id == id) transform(it).also { next -> require(next.id == id) } else it
    })
    fun files(id: String, state: FilesState): FileWorkspace {
        val old = tab(id) ?: return this
        val spacesChanged = old.files.spaces != state.spaces
        return copy(tabs = tabs.map {
            when { it.id == id -> it.copy(files = state)
                spacesChanged -> it.copy(files = it.files.copy(spaces = state.spaces))
                else -> it }
        })
    }
    fun focus(id: String): FileWorkspace {
        require(tab(id) != null)
        return when (id) {
            left, right -> copy(active = id)
            else -> if (active == right) copy(right = id, active = id) else copy(left = id, active = id)
        }
    }
    fun add(duplicate: Boolean = false): FileWorkspace {
        require(tabs.size < MAX_FILE_TABS)
        val id = "files-$nextId"
        val source = current
        val state = if (duplicate) FilesState(spaces = source.files.spaces, trail = source.files.trail.toList())
            else FilesState(spaces = source.files.spaces)
        val tab = FileTab(id, state, if (duplicate) source.options else FileListOptions(), if (duplicate) source.view else null)
        return copy(tabs = tabs + tab, nextId = nextId + 1).focus(id)
    }
    fun split(enabled: Boolean): FileWorkspace {
        if (!enabled) return copy(left = active, right = null)
        if (right != null) return this
        val other = tabs.firstOrNull { it.id != left }
        if (other != null) return copy(right = other.id)
        val added = add(duplicate = true)
        return added.copy(left = left, right = added.active, active = active)
    }
    fun moveToOtherPane(id: String): FileWorkspace {
        require(tab(id) != null)
        val two = split(true)
        return when (id) {
            two.left -> two.copy(left = requireNotNull(two.right), right = id, active = id)
            two.right -> two.copy(left = id, right = two.left, active = id)
            else -> if (two.active == two.left) two.copy(right = id, active = id) else two.copy(left = id, active = id)
        }
    }
    fun keep(ids: Set<String>): FileWorkspace {
        val kept = tabs.filter { it.id in ids }
        require(kept.isNotEmpty())
        val nextLeft = if (left in ids) left else kept.first().id
        val nextRight = right?.takeIf { it in ids && it != nextLeft }
        val nextActive = active.takeIf { it == nextLeft || it == nextRight } ?: nextLeft
        return copy(tabs = kept, left = nextLeft, right = nextRight, active = nextActive)
    }
    fun close(id: String): FileWorkspace {
        if (tabs.size == 1) return this
        return keep(tabs.map { it.id }.toSet() - id)
    }
    fun withoutSpace(id: String) = copy(tabs = tabs.map {
        val state = it.files
        it.copy(files = if (state.folder?.storageId == id) FilesState(spaces = state.spaces.filterNot { space -> space.id == id })
            else state.copy(spaces = state.spaces.filterNot { space -> space.id == id }))
    })
}
