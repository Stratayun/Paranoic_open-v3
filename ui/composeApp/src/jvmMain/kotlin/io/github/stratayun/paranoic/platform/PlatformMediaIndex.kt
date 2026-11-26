package io.github.stratayun.paranoic.platform

/**
 * The host's file index (Android MediaStore, owner 26.11): categories open at once
 * instead of walking every folder. Input: an absolute volume root, file extensions and
 * a result limit; output: files under that root, newest first, or null when the
 * index is unavailable (then the app walks folders as before). Only names and sizes
 * come from here: every operation still goes through Core.
 */
object PlatformMediaIndex {
    data class Entry(val path: String, val bytes: Long, val modifiedMs: Long)
    @Volatile var query: ((root: String, extensions: Set<String>, limit: Int) -> List<Entry>?)? = null
}
