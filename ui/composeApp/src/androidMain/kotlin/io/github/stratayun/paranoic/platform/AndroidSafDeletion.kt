package io.github.stratayun.paranoic.platform

import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.storage.StorageManager
import java.io.File
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.system.Os
import android.system.OsConstants
import java.io.Closeable
import io.github.stratayun.paranoic.nativecore.safDeleteAt

/** Local provider only, explicit direct-storage permission, pinned no-follow parents. */
internal class AndroidSafDeletion(private val roots: () -> Map<String, String>) {
    private fun treeId(tree: Uri): String {
        if (tree.scheme != "content" || tree.authority != "com.android.externalstorage.documents" ||
            tree.query != null || tree.fragment != null || tree.pathSegments.size != 2 || tree.pathSegments[0] != "tree")
            throw UnsupportedOperationException("provider has no atomic deletion path")
        return DocumentsContract.getTreeDocumentId(tree)
    }

    fun supported(tree: Uri): Boolean = try {
        val id = ExternalStorageDeletePath.parse(treeId(tree))
        val root = roots()[id.volume] ?: throw UnsupportedOperationException("direct storage access required")
        PinnedDirectory.open(root, id.parts).use { }
        true
    } catch (_: Exception) { false }

    fun delete(tree: Uri, document: Uri, directory: Boolean) {
        val treeId = treeId(tree)
        require(document.scheme == tree.scheme && document.authority == tree.authority &&
            document.query == null && document.fragment == null &&
            DocumentsContract.getTreeDocumentId(document) == treeId) { "Foreign document URI" }
        val target = ExternalStorageDeletePath.target(treeId, DocumentsContract.getDocumentId(document))
        val root = roots()[target.volume] ?: throw UnsupportedOperationException("direct storage access required")
        PinnedDirectory.open(root, target.parts.dropLast(1)).use { it.delete(target.parts.last(), directory) }
    }

    companion object {
        /** System-reported roots only. Caller must require direct-storage permission. */
        fun volumeRoots(context: Context): Map<String, String> {
            val manager = context.getSystemService(StorageManager::class.java)
            val olderRoots = if (Build.VERSION.SDK_INT < 30)
                context.getExternalFilesDirs(null).filterNotNull().mapNotNull {
                    it.absolutePath.substringBefore("/Android/data/", "").takeIf(String::isNotEmpty)
                } else emptyList()
            val entries = manager.storageVolumes.filter { it.state == Environment.MEDIA_MOUNTED }.mapNotNull { volume ->
                val path = if (Build.VERSION.SDK_INT >= 30) volume.directory?.absolutePath else
                    olderRoots.singleOrNull { manager.getStorageVolume(File(it)) == volume }
                val id = if (volume.isPrimary) "primary" else volume.uuid?.lowercase()
                if (path == null || id == null) null else id to path
            }
            // An ambiguous volume ID is never a deletion target.
            return entries.groupBy({ it.first }, { it.second }).mapNotNull { (id, paths) ->
                paths.singleOrNull()?.let { id to it }
            }.toMap()
        }
    }

    internal class PinnedDirectory private constructor(private val descriptor: ParcelFileDescriptor) : Closeable {
        private fun child(name: String): String {
            require(ExternalStorageDeletePath.part(name)) { "Invalid filename" }
            return "/proc/self/fd/${descriptor.fd}/$name"
        }
        fun delete(name: String, directory: Boolean) {
            child(name) // validate before crossing the native boundary
            // Each syscall enforces its type atomically. rmdir refuses newly
            // appeared children; unlink refuses any directory, including a swap.
            safDeleteAt(descriptor.fd, name, directory)
        }
        override fun close() = descriptor.close()

        companion object {
            fun open(root: String, parts: List<String>): PinnedDirectory {
                require(root.startsWith('/')) { "Expected a mounted volume root" }
                var current = openOne(root)
                try {
                    for (part in parts) {
                        val next = openOne(current.child(part))
                        try { current.close() } catch (error: Exception) { next.close(); throw error }
                        current = next
                    }
                    return current
                } catch (error: Exception) { current.close(); throw error }
            }
            private fun openOne(path: String): PinnedDirectory {
                val raw = Os.open(path, OsConstants.O_RDONLY or OsConstants.O_NONBLOCK or
                    OsConstants.O_NOFOLLOW or OsConstants.O_CLOEXEC, 0)
                try {
                    check(OsConstants.S_ISDIR(Os.fstat(raw).st_mode)) { "Not a directory" }
                    return PinnedDirectory(ParcelFileDescriptor.dup(raw))
                }
                finally { Os.close(raw) }
            }
        }
    }
}
