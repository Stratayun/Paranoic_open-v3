package io.github.stratayun.paranoic.platform

import java.awt.Toolkit
import java.awt.datatransfer.DataFlavor
import java.awt.datatransfer.StringSelection
import java.nio.file.Files
import java.nio.file.Path
import java.security.MessageDigest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Audit 26.11, Windows. Temporary files of this process (the JavaFX media pipeline, JNA,
 * PDFium extraction) go to a private folder under the app data instead of the shared %TEMP%;
 * it is emptied at start and at exit, so a played vault video cannot linger there.
 */
internal object DesktopPrivateTemp {
    /** This process's own folder; removed at exit. */
    @Volatile var current: Path? = null
    private var lock: java.nio.channels.FileLock? = null

    /**
     * Audit A53: every process gets its own `tmp/<random>` folder, held by a lock file. At start
     * only folders whose lock is free (their process has ended) are removed, so a second launch
     * never deletes files of a running one.
     */
    fun install(appRoot: Path): Path? = try {
        val base = appRoot.resolve("tmp")
        Files.createDirectories(base)
        Files.list(base).use { entries -> entries.toList() }.forEach { stale ->
            if (!Files.isDirectory(stale)) return@forEach
            val channel = try { java.nio.channels.FileChannel.open(stale.resolve(".lock"),
                java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.WRITE) } catch (_: Exception) { null }
            val free = try { channel?.tryLock() } catch (_: Exception) { null }
            if (free != null) { free.release(); channel?.close(); wipe(stale); try { Files.deleteIfExists(stale) } catch (_: Exception) {} }
            else channel?.close()
        }
        val dir = base.resolve(java.util.UUID.randomUUID().toString())
        Files.createDirectories(dir)
        lock = java.nio.channels.FileChannel.open(dir.resolve(".lock"), java.nio.file.StandardOpenOption.CREATE,
            java.nio.file.StandardOpenOption.WRITE).tryLock()
        current = dir
        System.setProperty("java.io.tmpdir", dir.toString())
        if (System.getProperty("os.name").startsWith("Windows")) {
            // Native code (GLib/GStreamer in JavaFX) reads TMP/TEMP from the process environment.
            com.sun.jna.platform.win32.Kernel32.INSTANCE.SetEnvironmentVariable("TMP", dir.toString())
            com.sun.jna.platform.win32.Kernel32.INSTANCE.SetEnvironmentVariable("TEMP", dir.toString())
        }
        dir
    } catch (_: Throwable) { null }

    /** At exit: this process's folder only. */
    fun release() {
        val dir = current ?: return
        try { lock?.release(); lock?.channel()?.close() } catch (_: Exception) {}
        wipe(dir)
        try { Files.deleteIfExists(dir) } catch (_: Exception) {}
    }

    /** Best effort: files still open (a loaded DLL) stay until a later start. */
    fun wipe(dir: Path) {
        try {
            Files.walk(dir).use { paths ->
                paths.sorted(Comparator.reverseOrder()).filter { it != dir }.forEach { try { Files.deleteIfExists(it) } catch (_: Exception) {} }
            }
        } catch (_: Exception) {}
    }
}

/**
 * Audit 26.11, desktop clipboard: text copied in Paranoic is cleared after two minutes unless
 * something else was copied since. A change seen while the Paranoic window has focus is ours.
 * Only a hash of the clipboard text is kept. Windows clipboard history (Win+V) may still hold
 * a copy; that is a system setting outside the app.
 */
internal object DesktopClipboardHygiene {
    private const val CLEAR_AFTER_MS = 120_000L

    fun start(scope: CoroutineScope, focused: () -> Boolean) = scope.launch {
        var seen: String? = null
        var owned: String? = null
        var ownedAt = 0L
        while (isActive) {
            delay(1500)
            val current = textHash() ?: continue
            if (focused()) {
                if (seen != null && current != seen) { owned = current; ownedAt = System.currentTimeMillis() }
                seen = current
            }
            if (owned != null && System.currentTimeMillis() - ownedAt >= CLEAR_AFTER_MS) {
                if (current == owned) try { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(""), null) } catch (_: Exception) {}
                owned = null
                seen = textHash()
            }
        }
    }

    private fun textHash(): String? = try {
        val clipboard = Toolkit.getDefaultToolkit().systemClipboard
        if (!clipboard.isDataFlavorAvailable(DataFlavor.stringFlavor)) "" else {
            val text = clipboard.getData(DataFlavor.stringFlavor) as? String ?: ""
            MessageDigest.getInstance("SHA-256").digest(text.toByteArray()).joinToString("") { "%02x".format(it) }
        }
    } catch (_: Exception) { null }
}
