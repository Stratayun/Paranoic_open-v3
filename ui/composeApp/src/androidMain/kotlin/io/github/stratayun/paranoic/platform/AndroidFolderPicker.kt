package io.github.stratayun.paranoic.platform

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import io.github.stratayun.paranoic.bridge.*

/** SAF grants only. No listing/copy logic and no legacy JNI host. */
@Composable
fun rememberAndroidFolderPicker(onResult: (FolderPick) -> Unit): () -> Unit {
    val callback by rememberUpdatedState(onResult)
    val resolver = LocalContext.current.contentResolver
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode != Activity.RESULT_OK) callback(FolderPick.Cancelled)
        else {
            val intent = result.data
            val uri = intent?.data
            val flags = (intent?.flags ?: 0) and
                (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            if (uri == null || flags and Intent.FLAG_GRANT_READ_URI_PERMISSION == 0) callback(FolderPick.Failed)
            else try {
                resolver.takePersistableUriPermission(uri, flags)
                callback(FolderPick.Selected(PickedFolder(
                    FolderProvider.AndroidTree, uri.toString(),
                    writable = flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0,
                )))
            } catch (_: SecurityException) { callback(FolderPick.Failed) }
        }
    }
    return {
        try {
            launcher.launch(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or Intent.FLAG_GRANT_PREFIX_URI_PERMISSION))
        } catch (_: ActivityNotFoundException) { callback(FolderPick.Failed) }
        catch (_: SecurityException) { callback(FolderPick.Failed) }
    }
}

