package io.github.stratayun.paranoic.ui.screens.notes

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.CoreFilePickerDialog
import io.github.stratayun.paranoic.ui.components.CorePickKind
import io.github.stratayun.paranoic.ui.state.tr
import kotlinx.coroutines.*

internal class CoreNotesAttachmentPicker(private val core: CoreGateway) : NotesAttachmentPicker {
    private var target by mutableStateOf<String?>(null)
    override var pending by mutableStateOf<PendingNoteAttachment?>(null)
        private set
    override var fault by mutableStateOf<CoreFault?>(null)
        private set
    override val picking get() = target != null
    override val available get() = core.connected

    override fun pick(noteId: String) {
        if (!picking) {
            clear()
            target = noteId
        }
    }

    override fun clear() {
        target = null
        pending = null
        fault = null
    }

    @Composable
    fun Dialog(spaces: List<Space>) {
        val noteId = target ?: return
        CoreFilePickerDialog(core, spaces, tr("Добавить вложение", "Add attachment"), CorePickKind.Files,
            onDismiss = ::clear, onPick = { files ->
                val item = files.single()
                target = null
                if (item.bytes > NOTE_ATTACHMENT_MAX_BYTES.toULong()) fault = CoreFault.Limit
                else pending = PendingNoteAttachment(noteId, item.name, item.bytes.toLong(),
                    NoteAttachmentSource { read(item.location) })
            })
    }

    private suspend fun read(location: Location): CoreResult<ByteArray> {
        val opened = core.openMedia(location)
        if (opened is CoreResult.Failure) return opened
        val source = (opened as CoreResult.Value).value
        var bytes: ByteArray? = null
        var delivered = false
        try {
            val result = try {
                withContext(Dispatchers.IO) {
                    if (source.length !in 0L..NOTE_ATTACHMENT_MAX_BYTES.toLong())
                        return@withContext CoreResult.Failure(CoreFault.Limit)
                    val output = ByteArray(source.length.toInt()).also { bytes = it }
                    var offset = 0
                    while (offset < output.size) {
                        currentCoroutineContext().ensureActive()
                        val count = minOf(65_536, output.size - offset)
                        val part = source.readAt(offset.toLong(), count)
                        try {
                            if (part.isEmpty() || part.size > count)
                                return@withContext CoreResult.Failure(CoreFault.Changed)
                            part.copyInto(output, offset)
                            offset += part.size
                        } finally { part.fill(0) }
                    }
                    CoreResult.Value(output)
                }
            } finally { withContext(NonCancellable + Dispatchers.IO) { source.close() } }
            currentCoroutineContext().ensureActive()
            delivered = result is CoreResult.Value
            return result
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { return CoreResult.Failure(CoreFault.Unavailable) }
        finally { if (!delivered) bytes?.fill(0) }
    }
}
