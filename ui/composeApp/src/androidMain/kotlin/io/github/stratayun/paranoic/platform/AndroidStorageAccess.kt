package io.github.stratayun.paranoic.platform

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.storage.StorageManager
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import io.github.stratayun.paranoic.bridge.CoreGateway
import io.github.stratayun.paranoic.bridge.CoreResult
import io.github.stratayun.paranoic.bridge.FolderProvider
import io.github.stratayun.paranoic.bridge.PickedFolder
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.components.ExpressiveShapes
import io.github.stratayun.paranoic.ui.components.ShapedIcon
import io.github.stratayun.paranoic.ui.state.AppearanceSettings
import io.github.stratayun.paranoic.ui.theme.ParanoicTheme
import java.io.File

/** A mounted shared-storage volume: internal memory or a removable card. */
data class AndroidVolume(val path: String, val label: String, val removable: Boolean)

/**
 * Direct access to shared storage ("All files access", API 30+; legacy
 * READ/WRITE_EXTERNAL_STORAGE below). Volumes are registered in Core as
 * ordinary native grants (FolderProvider.AndroidVolume), so listing, copying
 * and encryption run through the Rust filesystem adapter instead of SAF.
 * SAF trees remain available for cloud document providers.
 */
object AndroidStorageAccess {
    private const val PREFS = "paranoic.storage_access"
    private const val KEY_ATTACHED = "auto_attached"

    fun granted(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= 30) Environment.isExternalStorageManager()
        else context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED

    fun settingsIntent(context: Context): Intent =
        Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}"))

    fun volumes(context: Context): List<AndroidVolume> {
        val manager = context.getSystemService(StorageManager::class.java)
        val found = if (Build.VERSION.SDK_INT >= 30) {
            manager.storageVolumes.filter { it.state == Environment.MEDIA_MOUNTED }.mapNotNull { volume ->
                val directory = volume.directory ?: return@mapNotNull null
                AndroidVolume(directory.absolutePath, label(context, volume.isPrimary, volume.getDescription(context)), volume.isRemovable)
            }
        } else {
            // Before API 30 StorageVolume has no directory: derive roots from the
            // app-specific external dirs (<root>/Android/data/<package>/files).
            context.getExternalFilesDirs(null).filterNotNull().mapNotNull { dir ->
                val root = dir.absolutePath.substringBefore("/Android/data/", "").ifEmpty { return@mapNotNull null }
                val primary = root == Environment.getExternalStorageDirectory().absolutePath
                val volume = manager.getStorageVolume(File(root))
                AndroidVolume(root, label(context, primary, volume?.getDescription(context)), !primary)
            }
        }
        return found.distinctBy { it.path }.sortedBy { it.removable }
    }

    private fun label(context: Context, primary: Boolean, description: String?): String =
        if (primary) internalLabel(context) else description?.takeIf { it.isNotBlank() } ?: "SD"

    private fun internalLabel(context: Context): String =
        if (context.resources.configuration.locales[0].language == "ru") "Внутренняя память" else "Internal storage"

    /** Free/total bytes of the mounted volume with this label. */
    fun usage(context: Context, label: String): io.github.stratayun.paranoic.ui.state.VolumeUsage? = try {
        volumes(context).firstOrNull { it.label == label }?.let {
            val stat = android.os.StatFs(it.path)
            io.github.stratayun.paranoic.ui.state.VolumeUsage(stat.availableBytes, stat.totalBytes)
        }
    } catch (_: Exception) { null }

    /** Label for a saved grant reference; null for anything that is not a current volume root. */
    fun labelFor(context: Context, reference: String): String? =
        volumes(context).firstOrNull { it.path == reference }?.label

    /**
     * Registers each mounted volume once. A volume the user later disconnects
     * is not re-added on the next start (remembered per path).
     */
    suspend fun attachVolumes(context: Context, core: CoreGateway) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val done = prefs.getStringSet(KEY_ATTACHED, emptySet()).orEmpty()
        val attached = done.toMutableSet()
        for (volume in volumes(context)) {
            if (volume.path in done) continue
            val result = core.attachFolder(PickedFolder(FolderProvider.AndroidVolume, volume.path, writable = true))
            if (result is CoreResult.Value) attached += volume.path
        }
        if (attached != done) prefs.edit().putStringSet(KEY_ATTACHED, attached).apply()
    }
}

/**
 * Asks for direct storage access before the main UI, once per app start
 * until granted. "Later" continues with SAF folders only.
 */
@Composable
fun AndroidStorageAccessGate(core: CoreGateway, appearance: AppearanceSettings, content: @Composable () -> Unit) {
    val context = LocalContext.current
    var granted by remember { mutableStateOf(AndroidStorageAccess.granted(context)) }
    var skipped by remember { mutableStateOf(false) }
    var ready by remember { mutableStateOf(false) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) granted = AndroidStorageAccess.granted(context)
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val legacy = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        granted = AndroidStorageAccess.granted(context)
    }
    LaunchedEffect(granted) {
        if (granted) AndroidStorageAccess.attachVolumes(context, core)
        ready = granted
    }
    when {
        ready || skipped -> content()
        granted -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        else -> ParanoicTheme(appearance) { StorageAccessRequest(
            onGrant = {
                if (Build.VERSION.SDK_INT >= 30) {
                    try { context.startActivity(AndroidStorageAccess.settingsIntent(context)) }
                    catch (_: ActivityNotFoundException) {
                        context.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                    }
                } else legacy.launch(arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE))
            },
            onLater = { skipped = true },
        ) }
    }
}

@Composable
private fun StorageAccessRequest(onGrant: () -> Unit, onLater: () -> Unit) {
    val ru = LocalContext.current.resources.configuration.locales[0].language == "ru"
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
        Column(
            Modifier.fillMaxSize().safeDrawingPadding().padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            ShapedIcon(AppSymbol.Folder, ExpressiveShapes.Cookie9, MaterialTheme.colorScheme.primaryContainer,
                MaterialTheme.colorScheme.onPrimaryContainer, size = 112.dp)
            Spacer(Modifier.height(28.dp))
            Text(if (ru) "Доступ к файлам" else "File access", style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
            Spacer(Modifier.height(12.dp))
            Text(
                if (ru) "Чтобы работать с внутренней памятью и SD-картой напрямую, как в обычном файловом менеджере, разрешите Paranoic доступ ко всем файлам. Без этого доступны только папки, выбранные через системный диалог."
                else "To work with internal storage and SD cards directly, like a regular file manager, allow Paranoic to access all files. Without it, only folders picked in the system dialog are available.",
                style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(32.dp))
            Button(onGrant, Modifier.fillMaxWidth().height(56.dp), shape = RoundedCornerShape(28.dp)) {
                Text(if (ru) "Разрешить доступ" else "Allow access", style = MaterialTheme.typography.titleMedium)
            }
            Spacer(Modifier.height(8.dp))
            TextButton(onLater, Modifier.fillMaxWidth().height(48.dp)) {
                Text(if (ru) "Позже — только выбранные папки" else "Later — picked folders only")
            }
        }
    }
}
