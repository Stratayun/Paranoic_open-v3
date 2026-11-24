package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.FoundVault
import io.github.stratayun.paranoic.bridge.Location
import io.github.stratayun.paranoic.bridge.VaultKind

/** References only: a bookmark grants no permission and contains no credentials. */
data class SavedVault(val location: Location, val kind: VaultKind, val title: String,
    val readOnly: Boolean = false, val idleLockSeconds: Int? = null) {
    init {
        require(location.storageId.isNotBlank() && location.storageId.length <= 128)
        require(location.path.length <= 4096)
        require(validVaultTitle(title))
        require(idleLockSeconds == null || idleLockSeconds in VAULT_IDLE_LOCK_CHOICES)
    }
    fun found() = FoundVault(location, title, kind)
}
fun validVaultTitle(value: String) = value.isNotBlank() && value.length <= 120 && value.none { it.isISOControl() }
val VAULT_IDLE_LOCK_CHOICES = setOf(60, 300, 900)
/** Unreadable preferences must never silently grant a writable session. */
fun canWriteOpenedVault(physicalWritable: Boolean, saved: SavedVault?, metadataLoaded: Boolean, metadataFailed: Boolean): Boolean =
    physicalWritable && metadataLoaded && !metadataFailed && saved?.readOnly != true

/** Immutable operations keep duplicate handling and undo independent of Compose. */
data class VaultCatalogSnapshot(val entries: List<SavedVault> = emptyList(), val defaultIdleLockSeconds: Int = 300) {
    init {
        require(entries.size <= 128)
        require(entries.map { it.location }.distinct().size == entries.size)
        require(defaultIdleLockSeconds in VAULT_IDLE_LOCK_CHOICES)
    }
    fun remember(vault: SavedVault): VaultCatalogSnapshot {
        val existing = entries.indexOfFirst { it.location == vault.location }
        // User titles survive rediscovery. A changed format must be reviewed anew.
        if (existing >= 0) {
            require(entries[existing].kind == vault.kind)
            return this
        }
        return copy(entries = entries + vault)
    }
    fun rename(location: Location, title: String): VaultCatalogSnapshot {
        require(validVaultTitle(title))
        require(entries.any { it.location == location })
        return copy(entries = entries.map { if (it.location == location) it.copy(title = title.trim()) else it })
    }
    /** Rebinding is a user decision after successful opening, never a scan side effect. */
    fun relocate(original: SavedVault, destination: Location, kind: VaultKind): VaultCatalogSnapshot {
        require(kind == original.kind)
        val index = entries.indexOfFirst { it.location == original.location }
        require(index >= 0 && entries[index] == original)
        require(entries.none { it.location == destination && it.location != original.location })
        return copy(entries = entries.mapIndexed { i, entry -> if (i == index) entry.copy(location = destination) else entry })
    }
    fun remove(location: Location) = copy(entries = entries.filterNot { it.location == location })
    fun configure(location: Location, readOnly: Boolean, idleLockSeconds: Int?): VaultCatalogSnapshot {
        require(entries.any { it.location == location })
        return copy(entries = entries.map { if (it.location == location) it.copy(readOnly = readOnly, idleLockSeconds = idleLockSeconds) else it })
    }
    fun restore(vault: SavedVault, index: Int): VaultCatalogSnapshot {
        require(entries.none { it.location == vault.location })
        val next = entries.toMutableList().apply { add(index.coerceIn(0, size), vault) }
        return copy(entries = next)
    }
}
interface VaultCatalogStore {
    /** IO must complete or throw. Corruption is never interpreted as empty. */
    suspend fun read(): VaultCatalogSnapshot
    suspend fun write(snapshot: VaultCatalogSnapshot)
}
/** Explicit default for previews/tests; production roots inject protected stores. */
class MemoryVaultCatalogStore : VaultCatalogStore {
    private var snapshot = VaultCatalogSnapshot()
    override suspend fun read() = snapshot
    override suspend fun write(snapshot: VaultCatalogSnapshot) { this.snapshot = snapshot }
}
