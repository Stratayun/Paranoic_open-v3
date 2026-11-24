package io.github.stratayun.paranoic.ui.screens.notes

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.*

/** No selection/clipboard, intents, share/export path or plaintext cache. */
@Composable
internal fun NoteAttachmentPreview(core: CoreGateway, attachment: NoteAttachment,
    image: @Composable (ByteArray, Modifier) -> Unit, onClose: () -> Unit) {
    var bytes by remember(attachment.id) { mutableStateOf<ByteArray?>(null) }
    var text by remember(attachment.id) { mutableStateOf<String?>(null) }
    var isImage by remember(attachment.id) { mutableStateOf(false) }
    var truncated by remember(attachment.id) { mutableStateOf(false) }
    var fault by remember(attachment.id) { mutableStateOf<CoreFault?>(null) }
    LaunchedEffect(core, attachment.id) {
        var owned: ByteArray? = null
        try {
            when (val result = core.notesAttachmentRead(attachment.id)) {
                is CoreResult.Failure -> fault = result.fault
                is CoreResult.Value -> {
                    val data = result.value
                    owned = data
                    ensureActive()
                    if (data.size > NOTE_ATTACHMENT_MAX_BYTES) fault = CoreFault.Limit
                    else if (attachmentImage(data)) { isImage = true; bytes = data }
                    else {
                        val decoded = withContext(Dispatchers.Default) { attachmentText(data) }
                        text = decoded.first
                        truncated = decoded.second
                        data.fill(0)
                    }
                }
            }
            awaitCancellation() // Keep the owned image array until the preview closes.
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unsupported }
        finally { owned?.fill(0); bytes = null; text = null }
    }
    DisposableEffect(attachment.id) { onDispose { bytes?.fill(0); bytes = null; text = null } }
    Dialog(onDismissRequest = onClose) {
        Surface(shape = MaterialTheme.shapes.large) {
            Column(Modifier.fillMaxWidth().heightIn(max = 600.dp).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(attachment.name), style = MaterialTheme.typography.titleMedium, maxLines = 2)
                Text(tr("Просмотр только внутри заметок", "View inside notes only"), style = MaterialTheme.typography.bodySmall)
                fault?.let { FaultNotice(it) }
                if (truncated) Text(tr("Показано начало текста (до 128 КиБ).", "Showing the beginning of the text (up to 128 KiB)."))
                val data = bytes
                val content = text
                when {
                    fault != null -> Unit
                    isImage && data != null -> image(data, Modifier.weight(1f, fill = false).fillMaxWidth())
                    content != null -> Text(content, Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()))
                    else -> CircularProgressIndicator()
                }
                TextButton(onClick = onClose) { Text(tr("Закрыть", "Close")) }
            }
        }
    }
}

private fun attachmentImage(bytes: ByteArray): Boolean {
    fun at(index: Int) = bytes.getOrNull(index)?.toInt()?.and(255)
    return (at(0) == 137 && at(1) == 80 && at(2) == 78 && at(3) == 71) ||
        (at(0) == 255 && at(1) == 216 && at(2) == 255) ||
        (at(0) == 71 && at(1) == 73 && at(2) == 70 && at(3) == 56) ||
        (at(0) == 82 && at(1) == 73 && at(2) == 70 && at(3) == 70 &&
            at(8) == 87 && at(9) == 69 && at(10) == 66 && at(11) == 80)
}

private fun attachmentText(bytes: ByteArray): Pair<String, Boolean> {
    val limit = minOf(bytes.size, 128 * 1024)
    val bomLe = bytes.size >= 2 && bytes[0] == 0xff.toByte() && bytes[1] == 0xfe.toByte()
    val bomBe = bytes.size >= 2 && bytes[0] == 0xfe.toByte() && bytes[1] == 0xff.toByte()
    val text = if (bomLe || bomBe) {
        val chars = CharArray((limit - 2) / 2)
        try {
            chars.indices.forEach { i ->
                val a = bytes[2 + i * 2].toInt() and 255
                val b = bytes[3 + i * 2].toInt() and 255
                chars[i] = (if (bomLe) a or (b shl 8) else (a shl 8) or b).toChar()
            }
            chars.concatToString()
        } finally { chars.fill('\u0000') }
    } else {
        var end = limit
        if (end < bytes.size) while (end > 0 && (bytes[end].toInt() and 0xc0) == 0x80) end--
        bytes.decodeToString(0, end, throwOnInvalidSequence = true).removePrefix("\uFEFF")
    }
    require(text.none { it.isISOControl() && it !in "\n\r\t" })
    return text to (limit < bytes.size)
}
