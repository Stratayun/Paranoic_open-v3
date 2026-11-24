package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.CoreFault
import io.github.stratayun.paranoic.bridge.VaultKind

/** Explicit OS-picker handoff only. In memory, never saveable; contains no credentials.
 * Ordinary backgrounding still discards the vault screen and its password dialogs. */
data class VaultFolderRequest(
    val creating: Boolean,
    val kind: VaultKind,
    val trail: List<FolderNode>,
    val relocating: SavedVault? = null,
) {
    init { require(!creating || relocating == null) }
}

data class VaultFolderReturn(val request: VaultFolderRequest, val fault: CoreFault? = null)

/** One OS selection/attachment at a time. A completed or stale reply cannot replace
 * a later request, even if both requests have equal contents. UI thread only. */
class VaultFolderPickerFlow {
    var pending: VaultFolderRequest? = null
        private set
    private var selectionReceived = false

    fun begin(request: VaultFolderRequest): Boolean {
        if (pending != null) return false
        pending = request.copy(trail = request.trail.toList())
        selectionReceived = false
        return true
    }

    fun acceptSelection(): VaultFolderRequest? {
        val request = pending ?: return null
        if (selectionReceived) return null
        selectionReceived = true
        return request
    }

    fun finish(request: VaultFolderRequest, folder: FolderNode? = null, fault: CoreFault? = null): VaultFolderReturn? {
        if (pending !== request) return null
        pending = null
        selectionReceived = false
        return VaultFolderReturn(request.copy(trail = if (folder != null && fault == null) listOf(folder) else request.trail), fault)
    }
}
