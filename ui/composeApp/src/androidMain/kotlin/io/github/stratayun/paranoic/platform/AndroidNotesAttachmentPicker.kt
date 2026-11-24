package io.github.stratayun.paranoic.platform

import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.tr
import kotlinx.coroutines.*

/** Remembered outside the app auth gate: the picker itself may cause ON_STOP and lock notes. */
@Composable
fun rememberAndroidNotesAttachmentPicker(): NotesAttachmentPicker {
    val resolver = LocalContext.current.contentResolver
    val scope = rememberCoroutineScope()
    val picker = remember { AndroidNotePicker() }
    val fallbackName = tr("вложение", "attachment")
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        val target = picker.target
        if (uri == null || target == null) { picker.picking = false; picker.target = null }
        else scope.launch {
            try {
                val metadata = withContext(Dispatchers.IO) {
                    var name = fallbackName
                    var size: Long? = null
                    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
                        if (cursor.moveToFirst()) {
                            val n = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                            val s = cursor.getColumnIndex(OpenableColumns.SIZE)
                            if (n >= 0 && !cursor.isNull(n)) name = cursor.getString(n).take(255)
                            if (s >= 0 && !cursor.isNull(s)) size = cursor.getLong(s).takeIf { it >= 0 }
                        }
                    }
                    name to size
                }
                if ((metadata.second ?: 0) > NOTE_ATTACHMENT_MAX_BYTES) picker.fault = CoreFault.Limit
                else picker.pending = PendingNoteAttachment(target, metadata.first, metadata.second,
                    NoteAttachmentSource { readNoteAttachment { resolver.openInputStream(uri) ?: error("Unavailable") } })
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { picker.fault = CoreFault.Unavailable }
            finally { picker.picking = false; picker.target = null }
        }
    }
    SideEffect { picker.launch = { launcher.launch("*/*") } }
    return picker
}
private class AndroidNotePicker : NotesAttachmentPicker {
    override var pending by mutableStateOf<PendingNoteAttachment?>(null)
    override var fault by mutableStateOf<CoreFault?>(null)
    override var picking by mutableStateOf(false)
    var target: String? = null
    var launch: () -> Unit = {}
    override fun pick(noteId: String) {
        if (picking) return
        clear()
        target = noteId
        picking = true
        try { launch() } catch (_: Exception) { picking = false; target = null; fault = CoreFault.Unavailable }
    }
    override fun clear() { pending = null; fault = null }
}
