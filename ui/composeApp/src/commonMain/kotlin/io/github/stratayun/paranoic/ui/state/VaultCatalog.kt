package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.Location
import kotlinx.coroutines.*

class VaultCatalog(private val store: VaultCatalogStore, private val scope: CoroutineScope) {
    var snapshot by mutableStateOf(VaultCatalogSnapshot()); private set
    var loaded by mutableStateOf(false); private set
    var busy by mutableStateOf(false); private set
    var failed by mutableStateOf(false); private set
    var removed by mutableStateOf<SavedVault?>(null); private set
    private var removedIndex = 0
    private var pendingRemember: SavedVault? = null
    private fun finished() {
        busy = false
        val pending = pendingRemember; pendingRemember = null
        if (pending != null && loaded && !failed) remember(pending)
    }
    init { load() }
    fun load() {
        if (busy) return
        busy = true; failed = false
        scope.launch {
            try { snapshot = store.read(); loaded = true; removed = null }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { failed = true; loaded = false }
            finally { finished() }
        }
    }
    private fun change(transform: (VaultCatalogSnapshot) -> VaultCatalogSnapshot, committed: () -> Unit = {}) {
        if (!loaded || busy || failed) return
        busy = true
        scope.launch {
            try {
                val next = transform(snapshot)
                // Await the physical write so late cancellation cannot leave the
                // screen claiming a decision that was never stored.
                withContext(NonCancellable) { store.write(next); snapshot = next; committed() }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { failed = true }
            finally { finished() }
        }
    }
    fun remember(vault: SavedVault) {
        if (snapshot.entries.any { it.location == vault.location && it.kind == vault.kind }) return
        if (busy) { pendingRemember = vault; return }
        change({ it.remember(vault) })
    }
    fun rename(location: Location, title: String) = change({ it.rename(location, title) })
    fun configure(location: Location, readOnly: Boolean, idleLockSeconds: Int?) = change({ it.configure(location, readOnly, idleLockSeconds) })
    fun settings(location: Location, title: String, readOnly: Boolean, idleLockSeconds: Int?) =
        change({ it.rename(location, title).configure(location, readOnly, idleLockSeconds) })
    fun defaultIdleLock(seconds: Int) = change({ it.copy(defaultIdleLockSeconds = seconds) })
    fun relocate(original: SavedVault, destination: Location, kind: io.github.stratayun.paranoic.bridge.VaultKind) =
        change({ it.relocate(original, destination, kind) }) { removed = null }
    fun remove(location: Location) {
        val entry = snapshot.entries.singleOrNull { it.location == location } ?: return
        val index = snapshot.entries.indexOf(entry)
        change({ it.remove(location) }) { removed = entry; removedIndex = index }
    }
    fun undo() {
        val entry = removed ?: return
        change({ it.restore(entry, removedIndex) }) { removed = null }
    }
}
