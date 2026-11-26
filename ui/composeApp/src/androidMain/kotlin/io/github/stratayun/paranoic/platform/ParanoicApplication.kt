package io.github.stratayun.paranoic.platform

import android.app.Application
import android.os.Build
import android.content.Intent
import android.net.Uri
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import java.io.File

/** One process runtime: Activity recreation cannot race two journal owners.
 * A failed start is not cached: the next request (Retry) starts again. */
class ParanoicApplication : Application() {
    internal val diagnostics by lazy {
        JvmDiagnosticEnvironment(noBackupFilesDir.toPath().resolve("last-jvm-crash-v1.bin")) {
            @Suppress("DEPRECATION")
            val version = packageManager.getPackageInfo(packageName, 0).versionName ?: "Unavailable"
            io.github.stratayun.paranoic.ui.state.DiagnosticPlatformFacts("Android", "${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
                "${Build.MANUFACTURER} ${Build.MODEL}", version)
        }
    }
    override fun onCreate() {
        super.onCreate()
        // Android decodes every WebP variant, HEIC and AVIF; used only when the core decoder refuses a format.
        PlatformImageFallback.decode = { bytes, side ->
            val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
            android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            var sample = 1
            while (bounds.outWidth / (sample * 2) >= side && bounds.outHeight / (sample * 2) >= side) sample *= 2
            android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, android.graphics.BitmapFactory.Options().apply { inSampleSize = sample })
                ?.let { full ->
                    val scale = minOf(1f, side.toFloat() / maxOf(full.width, full.height))
                    val fitted = if (scale < 1f) android.graphics.Bitmap.createScaledBitmap(full, (full.width * scale).toInt().coerceAtLeast(1),
                        (full.height * scale).toInt().coerceAtLeast(1), true) else full
                    val out = java.io.ByteArrayOutputStream()
                    fitted.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out)
                    if (fitted !== full) fitted.recycle(); full.recycle()
                    out.toByteArray()
                }
        }
        // The isolated decoder process must not touch the app's private records.
        if (android.os.Process.myUid() == applicationInfo.uid) diagnostics.install()
    }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    val security by lazy { AndroidSecurityState(this) }
    val notesQuickKey by lazy { AndroidNotesQuickKey() }
    private var started: Deferred<NativeCoreGateway>? = null

    @Synchronized
    fun runtime(): Deferred<NativeCoreGateway> {
        check(android.os.Process.myUid() == applicationInfo.uid) { "Core is forbidden in the isolated decoder process" }
        started?.let { current ->
            if (!current.isCompleted || current.getCompletionExceptionOrNull() == null) return current
        }
        return scope.async {
            NativeCoreGateway.open(
                File(noBackupFilesDir, "core-state").absolutePath,
                File(filesDir, "workspace").absolutePath,
                File(applicationInfo.nativeLibraryDir, "libpdfium.so").absolutePath,
                // Packaged as a "library" so Android installs it executable in nativeLibraryDir.
                File(applicationInfo.nativeLibraryDir, "libparanoic_pdf_worker.so").absolutePath,
                AndroidDocumentsHost(contentResolver), AndroidGrantStore(this@ParanoicApplication), AndroidRecordSealer(),
                releaseGrant = { grant -> if (grant.androidTree) releaseTree(grant.reference) },
                notesKey = AndroidNotesKey(),
                notesQuickKey = if (Build.VERSION.SDK_INT >= 30) notesQuickKey else null,
                documentWorkers = AndroidDocumentWorkers(this@ParanoicApplication),
                spaceLabel = { reference -> AndroidStorageAccess.labelFor(this@ParanoicApplication, reference) },
            )
        }.also { started = it }
    }

    /** Release exactly the persisted modes still held for this tree. */
    private fun releaseTree(reference: String) {
        val uri = Uri.parse(reference)
        val held = contentResolver.persistedUriPermissions.firstOrNull { it.uri == uri } ?: return
        val flags = (if (held.isReadPermission) Intent.FLAG_GRANT_READ_URI_PERMISSION else 0) or
            (if (held.isWritePermission) Intent.FLAG_GRANT_WRITE_URI_PERMISSION else 0)
        if (flags != 0) try { contentResolver.releasePersistableUriPermission(uri, flags) } catch (_: SecurityException) {}
    }
}
