package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*
import kotlinx.coroutines.*

/** A single writer publishes navigation changes only after durable storage succeeds. */
class PlaceCatalog(private val store: PlaceCatalogStore, private val scope: CoroutineScope) {
    var snapshot by mutableStateOf(PlaceCatalogSnapshot()); private set
    var loaded by mutableStateOf(false); private set
    var busy by mutableStateOf(false); private set
    var failed by mutableStateOf(false); private set
    var removed by mutableStateOf<PlaceRemoval?>(null); private set
    var removedTitle by mutableStateOf<String?>(null); private set

    init { load() }
    fun load() {
        if (busy) return
        busy = true
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                snapshot = store.read(); loaded = true; failed = false
                removed = null; removedTitle = null
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { failed = true; loaded = false }
            finally { busy = false }
        }
    }
    private fun change(transform: (PlaceCatalogSnapshot) -> PlaceCatalogSnapshot, committed: () -> Unit = {}) {
        if (!loaded || busy || failed) return
        busy = true
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                val next = transform(snapshot)
                withContext(NonCancellable) { store.write(next); snapshot = next; committed() }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { failed = true }
            finally { busy = false }
        }
    }
    fun title(storageId: String, fallback: String) = snapshot.find(storageId)?.title ?: fallback
    fun rename(storageId: String, title: String) = change({ it.rename(storageId, title) })
    fun hide(storageId: String, title: String) {
        var removal: PlaceRemoval? = null
        change({ it.hide(storageId).also { result -> removal = result.second }.first }) {
            removed = removal; removedTitle = title
        }
    }
    fun undo() {
        val removal = removed ?: return
        change({ it.undo(removal) }) { removed = null; removedTitle = null }
    }
    fun pin(bookmark: PlaceBookmark) = change({ it.pin(bookmark) })
    fun unpin(bookmark: PlaceBookmark) = change({ it.unpin(bookmark.storageId, bookmark.path) })
    fun show(storageId: String) = change({ it.show(storageId) }) {
        if (removed?.hidden?.storageId == storageId) { removed = null; removedTitle = null }
    }
}
