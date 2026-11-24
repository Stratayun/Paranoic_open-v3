package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*

/** Preliminary UX check only; Rust ResourcePath is authoritative. */
fun plausibleOperationName(name: String): Boolean = name.isNotBlank() && name !in listOf(".", "..") &&
    name.encodeToByteArray().size <= 255 && name.none { it == '/' || it == '\\' || it.isISOControl() }

/** UI hints only. Core rechecks capabilities, names, versions and containment at execution. */
fun List<Space>.spaceOf(location: Location): Space? = firstOrNull { it.root.storageId == location.storageId }

fun FileItem.canCopy(spaces: List<Space>): Boolean =
    kind in listOf(EntryKind.File, EntryKind.Directory) && location.path.isNotEmpty() &&
        spaces.spaceOf(location)?.let { it.root != location } == true

fun FileItem.canMove(spaces: List<Space>): Boolean = canCopy(spaces) &&
    spaces.spaceOf(location)?.let { it.writable && !(it.androidTree && kind == EntryKind.Directory) } == true

fun FileItem.canRename(spaces: List<Space>): Boolean = canCopy(spaces) &&
    spaces.spaceOf(location)?.let { it.writable && !it.androidTree } == true

fun FileItem.canDelete(spaces: List<Space>): Boolean = canMove(spaces)

/** Component boundary comparison, never an OS lookup or native operation path. */
fun FileItem.canTransferTo(parent: Location, mode: TransferMode, spaces: List<Space>): Boolean {
    if (!canCopy(spaces) || spaces.spaceOf(parent)?.writable != true) return false
    if (mode == TransferMode.Move) {
        if (!canMove(spaces)) return false
        if (location.storageId == parent.storageId && spaces.spaceOf(location)?.androidTree == true) return false
    }
    return kind != EntryKind.Directory || location.storageId != parent.storageId ||
        (parent.path != location.path && !parent.path.startsWith(location.path + "/"))
}

fun FileItem.isTarget(parent: Location, name: String): Boolean = location.storageId == parent.storageId &&
    location.path.substringBeforeLast('/', "") == parent.path && location.path.substringAfterLast('/') == name

fun FileItem.canReplace(target: FileItem?, parent: Location, name: String, spaces: List<Space>): Boolean =
    kind == EntryKind.File && target?.kind == EntryKind.File && target.location != location &&
        target.isTarget(parent, name) && spaces.spaceOf(parent)?.let { it.writable && !it.androidTree } == true
