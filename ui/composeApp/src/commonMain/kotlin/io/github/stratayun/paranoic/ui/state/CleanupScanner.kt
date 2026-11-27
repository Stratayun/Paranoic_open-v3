package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

@OptIn(kotlin.time.ExperimentalTime::class)
private fun nowMs(): ULong = kotlin.time.Clock.System.now().toEpochMilliseconds().toULong()

/** Exact duplicates: same size and same content hash. */
data class DuplicateGroup(val hash: String, val bytes: ULong, val items: List<FileItem>) {
    /** Space freed by keeping one copy. */
    val wasted: ULong get() = bytes * (items.size - 1).toULong()
}

enum class JunkKind { EmptyFiles, EmptyFolders, Temporary, Installers, Thumbnails }

data class JunkGroup(val kind: JunkKind, val items: List<FileItem>) {
    val bytes: ULong get() = items.fold(0uL) { sum, it -> sum + it.bytes }
}

data class CleanupState(
    val space: Space,
    val running: Boolean = true,
    val phase: String = "scan",
    val foldersScanned: Int = 0,
    val hashed: Int = 0,
    val toHash: Int = 0,
    val duplicates: List<DuplicateGroup> = emptyList(),
    val junk: List<JunkGroup> = emptyList(),
    /** Not junk, only candidates the owner reviews one by one (owner 26.11). */
    val large: List<FileItem> = emptyList(),
    val oldDownloads: List<FileItem> = emptyList(),
    val truncated: Boolean = false,
    val unreadable: Int = 0,
    val fault: CoreFault? = null,
)

/**
 * Read-only scan of one device place for duplicates and junk (owner 26.11).
 * Nothing is changed here; the screen moves what the owner picks to the trash.
 * Skips Android/ (app data), dot folders (except .thumbnails, reported as junk)
 * and the trash itself. Bounded in folders, files and bytes hashed.
 */
class CleanupScanner(private val core: CoreGateway, private val scopeProvider: () -> CoroutineScope) {
    var state by mutableStateOf<CleanupState?>(null); private set
    private var job: Job? = null

    fun scan(space: Space) {
        job?.cancel()
        state = CleanupState(space)
        job = scopeProvider().launch {
            val queue = ArrayDeque(listOf(space.root))
            val bySize = HashMap<ULong, MutableList<FileItem>>()
            val emptyFiles = mutableListOf<FileItem>(); val emptyFolders = mutableListOf<FileItem>()
            val temporary = mutableListOf<FileItem>(); val installers = mutableListOf<FileItem>(); val thumbnails = mutableListOf<FileItem>()
            val large = mutableListOf<FileItem>(); val oldDownloads = mutableListOf<FileItem>()
            val oldBefore = nowMs().let { if (it > OLD_AGE_MS) it - OLD_AGE_MS else 0uL }
            var scanned = 0; var unreadable = 0; var truncated = false; var files = 0
            val folderItems = HashMap<String, FileItem>()
            while (queue.isNotEmpty() && isActive) {
                if (scanned >= MAX_FOLDERS || files >= MAX_FILES) { truncated = true; break }
                val folder = queue.removeFirst()
                when (val listed = core.list(folder)) {
                    is CoreResult.Value -> {
                        val items = listed.value.items
                        if (items.isEmpty() && listed.value.hidden == 0L && folder != space.root) folderItems[folder.path]?.let { emptyFolders += it }
                        val inDownloads = folder.path.substringAfterLast('/').equals("Download", true) || folder.path.equals("Download", true)
                        for (item in items) {
                            val lower = item.name.lowercase()
                            when (item.kind) {
                                EntryKind.Directory -> when {
                                    lower == ".thumbnails" -> thumbnails += item
                                    item.name.startsWith(".") -> Unit
                                    folder == space.root && item.name == "Android" -> Unit
                                    else -> { folderItems[item.location.path] = item; queue.addLast(item.location) }
                                }
                                EntryKind.File -> {
                                    files++
                                    if (item.bytes >= LARGE_BYTES) large += item
                                    if (inDownloads && (item.modifiedMs ?: ULong.MAX_VALUE) < oldBefore) oldDownloads += item
                                    when {
                                        item.bytes == 0uL -> emptyFiles += item
                                        TEMP.any { lower.endsWith(it) } -> temporary += item
                                        inDownloads && lower.endsWith(".apk") -> installers += item
                                        item.bytes >= MIN_DUPLICATE -> bySize.getOrPut(item.bytes) { mutableListOf() } += item
                                    }
                                }
                                else -> Unit
                            }
                        }
                    }
                    is CoreResult.Failure -> if (folder == space.root) { state = state?.copy(fault = listed.fault, running = false); return@launch } else unreadable++
                }
                scanned++
                if (scanned % 25 == 0) state = state?.copy(foldersScanned = scanned)
            }
            val junk = listOf(JunkGroup(JunkKind.EmptyFiles, emptyFiles), JunkGroup(JunkKind.EmptyFolders, emptyFolders),
                JunkGroup(JunkKind.Temporary, temporary), JunkGroup(JunkKind.Installers, installers), JunkGroup(JunkKind.Thumbnails, thumbnails))
                .filter { it.items.isNotEmpty() }
            // Duplicates: only sizes seen more than once are compared, largest first; full reads stay within a byte budget.
            val candidates = bySize.filterValues { it.size > 1 }.entries.sortedByDescending { it.key }
            var budget = HASH_BUDGET
            val toHash = candidates.sumOf { it.value.size }
            state = state?.copy(phase = "hash", foldersScanned = scanned, toHash = toHash, junk = junk,
                large = large.sortedByDescending { it.bytes }.take(MAX_LIST),
                oldDownloads = oldDownloads.sortedBy { it.modifiedMs ?: 0uL }.take(MAX_LIST))
            val groups = mutableListOf<DuplicateGroup>()
            var hashed = 0
            for ((size, list) in candidates) {
                if (!isActive) return@launch
                // Stage 1: size + head/tail sample (128 KiB per file). Stage 2: full hash only where samples agree.
                val bySample = HashMap<String, MutableList<FileItem>>()
                for (item in list) {
                    if (!isActive) return@launch
                    val sample = if (size <= SAMPLE_COVERS_ALL) "all" else (core.fileSampleHash(item.location) as? CoreResult.Value)?.value
                    sample?.let { bySample.getOrPut(it) { mutableListOf() } += item }
                }
                val byHash = HashMap<String, MutableList<FileItem>>()
                for (same in bySample.values.filter { it.size > 1 }) {
                    // The byte budget limits full reads only; samples are cheap.
                    if (size * same.size.toULong() > budget) { truncated = true; continue }
                    budget -= size * same.size.toULong()
                    for (item in same) {
                        if (!isActive) return@launch
                        (core.fileHash(item.location) as? CoreResult.Value)?.value?.let { byHash.getOrPut(it) { mutableListOf() } += item }
                    }
                }
                hashed += list.size
                state = state?.copy(hashed = hashed)
                byHash.filterValues { it.size > 1 }.forEach { (hash, same) ->
                    groups += DuplicateGroup(hash, size, same.sortedBy { it.modifiedMs ?: ULong.MAX_VALUE })
                }
            }
            if (isActive) state = state?.copy(running = false, phase = "done", hashed = hashed,
                duplicates = groups.sortedByDescending { it.wasted }, truncated = truncated, unreadable = unreadable)
        }
    }

    fun close() { job?.cancel(); job = null; state = null }

    private companion object {
        val TEMP = listOf(".tmp", ".temp", ".part", ".crdownload", ".partial", "~")
        const val MAX_FOLDERS = 6000
        const val MAX_FILES = 60000
        const val MIN_DUPLICATE = 16_384uL
        const val HASH_BUDGET = 8_589_934_592uL
        /** 100 MiB and more. */
        const val LARGE_BYTES = 104_857_600uL
        /** Downloads untouched for half a year. */
        const val OLD_AGE_MS = 15_552_000_000uL
        const val MAX_LIST = 100
        /** Files this small are read whole by the full hash anyway. */
        const val SAMPLE_COVERS_ALL = 131_072uL
    }
}
