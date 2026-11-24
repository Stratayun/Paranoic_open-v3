package io.github.stratayun.paranoic.bridge

const val NOTE_ATTACHMENT_MAX_BYTES = 15 * 1024 * 1024
/** Import capability only. No output path, export or persistent access grant. */
fun interface NoteAttachmentSource { suspend fun read(): CoreResult<ByteArray> }
data class PendingNoteAttachment(val noteId: String, val name: String, val size: Long?, val source: NoteAttachmentSource)
interface NotesAttachmentPicker {
    val available: Boolean get() = true
    val pending: PendingNoteAttachment?
    val fault: CoreFault?
    val picking: Boolean
    fun pick(noteId: String)
    fun clear()
}
object NoNotesAttachmentPicker : NotesAttachmentPicker {
    override val available = false
    override val pending: PendingNoteAttachment? = null
    override val fault: CoreFault? = null
    override val picking = false
    override fun pick(noteId: String) = Unit
    override fun clear() = Unit
}
