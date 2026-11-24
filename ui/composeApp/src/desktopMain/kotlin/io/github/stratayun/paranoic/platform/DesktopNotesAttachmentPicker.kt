package io.github.stratayun.paranoic.platform

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.LocalUiLanguage
import io.github.stratayun.paranoic.ui.state.UiLanguage
import kotlinx.coroutines.*
import kotlinx.coroutines.swing.Swing
import java.nio.file.Files
import javax.swing.JFileChooser

@Composable
fun rememberDesktopNotesAttachmentPicker(owner: java.awt.Component? = null): NotesAttachmentPicker {
    val scope = rememberCoroutineScope()
    val picker = remember { DesktopNotePicker() }
    val title = tr("Добавить вложение в заметку", "Add a notes attachment")
    val approve = tr("Выбрать", "Select")
    val pickerLocale = java.util.Locale.forLanguageTag(if (LocalUiLanguage.current == UiLanguage.Russian) "ru" else "en")
    SideEffect {
        picker.launch = { noteId ->
            scope.launch {
                try {
                    val path = withContext(Dispatchers.Swing) {
                        val chooser = JFileChooser().apply {
                            this.locale = pickerLocale
                            updateUI()
                            dialogTitle = title
                            approveButtonText = approve
                            fileSelectionMode = JFileChooser.FILES_ONLY
                            isMultiSelectionEnabled = false
                        }
                        if (chooser.showOpenDialog(owner) == JFileChooser.APPROVE_OPTION) chooser.selectedFile?.toPath() else null
                    }
                    if (path != null) {
                        val size = withContext(Dispatchers.IO) { Files.size(path) }
                        if (size > NOTE_ATTACHMENT_MAX_BYTES) picker.fault = CoreFault.Limit
                        else picker.pending = PendingNoteAttachment(noteId, path.fileName.toString().take(255), size,
                            NoteAttachmentSource { readNoteAttachment { Files.newInputStream(path) } })
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { picker.fault = CoreFault.Unavailable }
                finally { picker.picking = false }
            }
        }
    }
    return picker
}
private class DesktopNotePicker : NotesAttachmentPicker {
    override var pending by mutableStateOf<PendingNoteAttachment?>(null)
    override var fault by mutableStateOf<CoreFault?>(null)
    override var picking by mutableStateOf(false)
    var launch: (String) -> Unit = {}
    override fun pick(noteId: String) {
        if (picking) return
        clear(); picking = true
        launch(noteId)
    }
    override fun clear() { pending = null; fault = null }
}
