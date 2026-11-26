package io.github.stratayun.paranoic.platform

import android.content.Context
import android.os.Build
import android.provider.MediaStore
import java.io.File

/**
 * MediaStore as the category index (owner 26.11). Android 11+ with all-files access only:
 * there the FUSE layer keeps the index in step with the app's own renames, moves and
 * deletes. Older versions and missing permission return null, and the app walks folders.
 */
internal object AndroidMediaIndex {
    fun query(context: Context, root: String, extensions: Set<String>, limit: Int): List<PlatformMediaIndex.Entry>? {
        if (Build.VERSION.SDK_INT < 30 || !AndroidStorageAccess.granted(context) || extensions.isEmpty()) return null
        val clean = extensions.filter { ext -> ext.isNotEmpty() && ext.all { it.isLetterOrDigit() } }
        if (clean.isEmpty()) return null
        @Suppress("DEPRECATION") val data = MediaStore.Files.FileColumns.DATA
        val selection = "(" + clean.joinToString(" OR ") { "$data LIKE ?" } + ") AND $data LIKE ? ESCAPE '\\'"
        val escapedRoot = root.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
        val args = (clean.map { "%.$it" } + "$escapedRoot/%").toTypedArray()
        val projection = arrayOf(data, MediaStore.Files.FileColumns.SIZE, MediaStore.Files.FileColumns.DATE_MODIFIED)
        val out = ArrayList<PlatformMediaIndex.Entry>()
        context.contentResolver.query(MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL), projection, selection, args,
            "${MediaStore.Files.FileColumns.DATE_MODIFIED} DESC")?.use { cursor ->
            while (cursor.moveToNext() && out.size < limit) {
                val path = cursor.getString(0) ?: continue
                if (!path.startsWith("$root/")) continue
                // The index can lag behind other apps: only files that exist now.
                val file = File(path)
                if (!file.isFile) continue
                out += PlatformMediaIndex.Entry(path, if (cursor.isNull(1)) file.length() else cursor.getLong(1),
                    if (cursor.isNull(2)) file.lastModified() else cursor.getLong(2) * 1000)
            }
        } ?: return null
        return out
    }
}
