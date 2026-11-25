package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** File kinds shown by the drawer's quick places, by extension. */
enum class FileCategory(val extensions: Set<String>) {
    Images(setOf("jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp", "avif")),
    Videos(setOf("mp4", "mkv", "webm", "mov", "avi", "3gp", "m4v")),
    Music(setOf("mp3", "flac", "ogg", "opus", "m4a", "wav", "aac", "wma")),
    Documents(setOf("pdf", "doc", "docx", "xls", "xlsx", "ods", "odt", "txt", "md", "rtf", "ppt", "pptx", "csv", "epub")),
}

data class CategoryState(val space: Space, val category: FileCategory, val items: List<FileItem> = emptyList(),
    val foldersScanned: Int = 0, val running: Boolean = true, val truncated: Boolean = false, val fault: CoreFault? = null)

/**
 * Read-only walk of one place through Core listings, collecting files of a
 * category. Bounded (folders and results) and skips dot folders and
 * Android/data|obb, which are large and not the user's own files.
 */
class CategoryBrowser(private val core: CoreGateway, private val scope: CoroutineScope) {
    var state by mutableStateOf<CategoryState?>(null); private set
    private var job: Job? = null

    fun open(space: Space, category: FileCategory) {
        close()
        state = CategoryState(space, category)
        job = scope.launch {
            val queue = ArrayDeque(listOf(space.root))
            val found = mutableListOf<FileItem>()
            var scanned = 0
            var truncated = false
            while (queue.isNotEmpty() && isActive) {
                if (scanned >= MAX_FOLDERS || found.size >= MAX_RESULTS) { truncated = true; break }
                val folder = queue.removeFirst()
                when (val listed = core.list(folder)) {
                    is CoreResult.Value -> for (item in listed.value.items) {
                        val name = item.name
                        when (item.kind) {
                            EntryKind.Directory -> if (!name.startsWith(".") && !(folder == space.root && name == "Android")) queue.addLast(item.location)
                            EntryKind.File -> if (name.substringAfterLast('.', "").lowercase() in category.extensions) found += item
                            else -> Unit
                        }
                    }
                    is CoreResult.Failure -> if (folder == space.root) { state = state?.copy(fault = listed.fault, running = false); return@launch }
                }
                scanned++
                if (scanned % 20 == 0) state = state?.copy(items = sorted(found), foldersScanned = scanned)
            }
            if (isActive) state = state?.copy(items = sorted(found), foldersScanned = scanned, running = false, truncated = truncated)
        }
    }

    fun close() { job?.cancel(); job = null; state = null }

    private fun sorted(items: List<FileItem>) = items.sortedByDescending { it.modifiedMs ?: 0uL }

    private companion object {
        const val MAX_FOLDERS = 4000
        const val MAX_RESULTS = 5000
    }
}
