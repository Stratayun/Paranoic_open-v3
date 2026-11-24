package io.github.stratayun.paranoic.ui.state

/** Navigation preferences only. They neither grant access nor rename disk folders. */
data class PlacePreference(val storageId: String, val title: String? = null, val hidden: Boolean = false) {
    init {
        require(storageId.isNotBlank() && storageId.length <= 128 && storageId.none { it.isISOControl() })
        require(title == null || validPlaceTitle(title))
    }
}

fun validPlaceTitle(value: String) = value.isNotBlank() && value.length <= 120 && value.none { it.isISOControl() }

data class PlaceRemoval(val before: PlacePreference?, val hidden: PlacePreference)

data class PlaceCatalogSnapshot(val entries: List<PlacePreference> = emptyList()) {
    init {
        require(entries.size <= 256)
        require(entries.map { it.storageId }.distinct().size == entries.size)
    }
    fun find(storageId: String) = entries.singleOrNull { it.storageId == storageId }
    private fun replace(storageId: String, entry: PlacePreference?): PlaceCatalogSnapshot {
        require(entry == null || entry.storageId == storageId)
        val index = entries.indexOfFirst { it.storageId == storageId }
        val next = entries.toMutableList()
        if (index >= 0) { if (entry == null) next.removeAt(index) else next[index] = entry }
        else if (entry != null) next.add(entry)
        return copy(entries = next)
    }
    fun rename(storageId: String, title: String): PlaceCatalogSnapshot {
        require(validPlaceTitle(title))
        return replace(storageId, (find(storageId) ?: PlacePreference(storageId)).copy(title = title.trim()))
    }
    fun hide(storageId: String): Pair<PlaceCatalogSnapshot, PlaceRemoval> {
        val before = find(storageId)
        val hidden = (before ?: PlacePreference(storageId)).copy(hidden = true)
        require(before?.hidden != true)
        return replace(storageId, hidden) to PlaceRemoval(before, hidden)
    }
    fun undo(removal: PlaceRemoval): PlaceCatalogSnapshot {
        // Do not overwrite a newer rename or visibility decision.
        require(find(removal.hidden.storageId) == removal.hidden)
        return replace(removal.hidden.storageId, removal.before)
    }
    fun show(storageId: String): PlaceCatalogSnapshot {
        val entry = find(storageId) ?: return this
        return replace(storageId, if (entry.title == null) null else entry.copy(hidden = false))
    }
}

interface PlaceCatalogStore {
    suspend fun read(): PlaceCatalogSnapshot
    suspend fun write(snapshot: PlaceCatalogSnapshot)
}

/** Preview/test default. Production roots supply an OS-protected store. */
class MemoryPlaceCatalogStore : PlaceCatalogStore {
    private var snapshot = PlaceCatalogSnapshot()
    override suspend fun read() = snapshot
    override suspend fun write(snapshot: PlaceCatalogSnapshot) { this.snapshot = snapshot }
}
