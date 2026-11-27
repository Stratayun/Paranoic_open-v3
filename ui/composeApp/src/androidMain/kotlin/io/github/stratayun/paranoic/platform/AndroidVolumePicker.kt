package io.github.stratayun.paranoic.platform

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import io.github.stratayun.paranoic.bridge.FolderPick
import io.github.stratayun.paranoic.bridge.PickedFolder
import io.github.stratayun.paranoic.ui.state.tr

/** Reconnect a mounted volume through the same Core entry point as auto-attach. */
@Composable
fun rememberAndroidVolumePicker(onResult: (FolderPick) -> Unit): () -> Unit {
    val context = LocalContext.current
    var volumes by remember { mutableStateOf<List<AndroidVolume>?>(null) }
    fun finish(result: FolderPick) {
        volumes = null
        onResult(result)
    }
    volumes?.let { available ->
        AlertDialog(
            onDismissRequest = { finish(FolderPick.Cancelled) },
            title = { Text(tr("Подключить накопитель", "Connect storage")) },
            text = {
                Column {
                    if (available.isEmpty()) Text(tr(
                        "Для выбора доступного накопителя разрешите доступ к файлам в настройках Android.",
                        "Allow file access in Android settings to choose a storage volume.",
                    ))
                    available.forEach { volume ->
                        TextButton(onClick = { finish(FolderPick.Selected(PickedFolder(volume.path))) }) {
                            Text(volume.label)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { finish(FolderPick.Cancelled) }) { Text(tr("Отмена", "Cancel")) }
            },
        )
    }
    return {
        try {
            volumes = if (AndroidStorageAccess.granted(context)) AndroidStorageAccess.volumes(context) else emptyList()
        } catch (_: Exception) {
            finish(FolderPick.Failed)
        }
    }
}
