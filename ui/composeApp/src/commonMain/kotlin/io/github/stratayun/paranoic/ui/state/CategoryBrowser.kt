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
    /** Search across every file kind. */
    All(emptySet()),
    Images(setOf("jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp", "avif")),
    Videos(setOf("mp4", "mkv", "webm", "mov", "avi", "3gp", "m4v")),
    Music(setOf("mp3", "flac", "ogg", "opus", "m4a", "wav", "aac", "wma")),
    Documents(setOf("pdf", "doc", "docx", "xls", "xlsx", "ods", "odt", "txt", "md", "rtf", "ppt", "pptx", "csv",
        "epub", "fb2", "djvu", "mobi", "azw3")),
    Archives(setOf("zip", "7z", "rar", "tar", "gz", "xz", "zst", "bz2", "age")),
}

data class CategoryState(val space: Space, val category: FileCategory, val query: String = "", val items: List<FileItem> = emptyList(),
    val foldersScanned: Int = 0, val running: Boolean = true, val truncated: Boolean = false, val fault: CoreFault? = null)

/**
 * Read-only walk of one place through Core listings, collecting files of a
 * category. Bounded (folders and results) and skips dot folders and
 * Android/data|obb, which are large and not the user's own files.
 */
class CategoryBrowser(private val core: CoreGateway, private val scope: CoroutineScope) {
    var state by mutableStateOf<CategoryState?>(null); private set
    private var job: Job? = null

    /** [query]: case-insensitive part of the name ("ё" = "е"); empty = every file of the category. */
    fun open(space: Space, category: FileCategory, query: String = "") {
        close()
        val needle = normalize(query.trim())
        state = CategoryState(space, category, query, running = category != FileCategory.All || needle.isNotEmpty())
        if (category == FileCategory.All && needle.isEmpty()) return
        fun matches(name: String) = (category == FileCategory.All || name.substringAfterLast('.', "").lowercase() in category.extensions) &&
            (needle.isEmpty() || normalize(name).contains(needle))
        job = scope.launch {
            val queue = ArrayDeque(listOf(space.root))
            val found = mutableListOf<FileItem>()
            var scanned = 0
            var truncated = false
            while (queue.isNotEmpty() && isActive) {
                if (scanned >= MAX_FOLDERS || found.size >= MAX_RESULTS) { truncated = true; break }
                val folder = queue.removeFirst()
                when (val listed = core.list(folder)) {
                    is CoreResult.Value -> {
                        val items = listed.value.items
                        // Book readers keep a cover image next to each PDF/EPUB ("Book.pdf" + "Book.jpg"):
                        // those are not the user's pictures.
                        val documentStems = if (category == FileCategory.Images) items.filter { it.kind == EntryKind.File &&
                            it.name.substringAfterLast('.', "").lowercase() in BOOKS }.map { it.name.substringBeforeLast('.').lowercase() }.toSet()
                            else emptySet()
                        for (item in items) {
                            val name = item.name
                            when (item.kind) {
                                EntryKind.Directory -> if (!skipFolder(name, folder == space.root)) queue.addLast(item.location)
                                EntryKind.File -> if (matches(name) && name.substringBeforeLast('.').lowercase() !in documentStems) found += item
                                else -> Unit
                            }
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

    private fun normalize(value: String) = value.lowercase().replace('ё', 'е')

    private fun sorted(items: List<FileItem>) = items.sortedByDescending { it.modifiedMs ?: 0uL }

    private fun skipFolder(name: String, atRoot: Boolean): Boolean {
        val lower = name.lowercase()
        return name.startsWith(".") || (atRoot && name == "Android") || lower in APP_FOLDERS
    }

    private companion object {
        /** App caches, thumbnails and cover stores: never the user's own media. */
        val APP_FOLDERS = setOf("cache", "caches", "thumbnails", "thumbs", "thumb", "covers", "cover", "tmp", "temp", "log", "logs")
        val BOOKS = setOf("pdf", "epub", "fb2", "djvu", "mobi", "azw3", "cbz", "cbr")
        const val MAX_FOLDERS = 4000
        const val MAX_RESULTS = 5000
    }
}
