package io.github.stratayun.paranoic.platform

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import io.github.stratayun.paranoic.ui.state.UiLanguage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.swing.Swing
import kotlinx.coroutines.withContext
import javax.swing.JFileChooser

/** Desktop OS selection, including Windows. Rust must revalidate ACLs and paths. */
@Composable
fun rememberDesktopFolderPicker(onResult: (FolderPick) -> Unit, owner: java.awt.Component? = null): () -> Unit {
    val callback by rememberUpdatedState(onResult)
    val scope = rememberCoroutineScope()
    val title = tr("Выбрать папку", "Choose folder")
    val approve = tr("Выбрать", "Select")
    val pickerLocale = java.util.Locale.forLanguageTag(if (LocalUiLanguage.current == UiLanguage.Russian) "ru" else "en")
    var picking by remember { mutableStateOf(false) }
    return {
        if (!picking) {
            picking = true
            scope.launch {
                try {
                    val result = withContext(Dispatchers.Swing) {
                        val chooser = JFileChooser().apply {
                            this.locale = pickerLocale
                            updateUI()
                            dialogTitle = title
                            approveButtonText = approve
                            fileSelectionMode = JFileChooser.DIRECTORIES_ONLY
                            isMultiSelectionEnabled = false
                            isAcceptAllFileFilterUsed = false
                        }
                        // Keep the picker in the main window's ownership tree:
                        // it must not look like a switch to another application.
                        if (chooser.showOpenDialog(owner) == JFileChooser.APPROVE_OPTION) {
                            val folder = chooser.selectedFile
                            if (folder == null) FolderPick.Failed else FolderPick.Selected(
                                PickedFolder(FolderProvider.DesktopDirectory, folder.absolutePath, writable = null))
                        } else FolderPick.Cancelled
                    }
                    callback(result)
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { callback(FolderPick.Failed) }
                finally { picking = false }
            }
        }
    }
}
