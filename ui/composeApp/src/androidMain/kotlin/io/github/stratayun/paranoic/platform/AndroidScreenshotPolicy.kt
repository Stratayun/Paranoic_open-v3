package io.github.stratayun.paranoic.platform

import android.content.Context
import android.util.AtomicFile
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.components.LabeledSwitch
import io.github.stratayun.paranoic.ui.state.announceChanges
import io.github.stratayun.paranoic.ui.state.tr
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream

/** Non-secret, local-only capture consent. Missing, unreadable or malformed = deny.
 * This preference deliberately does not share the theme store or Android backups. */
internal class AndroidScreenshotPolicy(private val setting: AtomicFile) {
    constructor(context: Context) : this(AtomicFile(File(context.noBackupFilesDir, "screen-capture-v1")))
    var allowed by mutableStateOf(readStrict() == true)
        private set

    private fun readStrict(): Boolean? = try {
        setting.openRead().use { input ->
            val byte = input.read()
            if (input.read() != -1) null else when (byte) { 0 -> false; 1 -> true; else -> null }
        }
    } catch (_: Exception) { null }

    private fun write(value: Boolean): Boolean {
        var output: FileOutputStream? = null
        return try {
            val stream = setting.startWrite()
            output = stream
            stream.write(if (value) 1 else 0)
            stream.fd.sync()
            setting.finishWrite(stream)
            output = null
            readStrict() == value
        } catch (_: Exception) {
            output?.let { runCatching { setting.failWrite(it) } }
            false
        }
    }

    /** Call from UI. Revocation applies before IO; granting waits for confirmed persistence. */
    suspend fun setAllowed(value: Boolean, apply: (Boolean) -> Unit): Boolean {
        if (!value) { allowed = false; apply(false) }
        val persisted = withContext(Dispatchers.IO) { write(value) }
        if (persisted) { allowed = value; apply(value) }
        return persisted
    }
}

@Composable
internal fun AndroidScreenshotSettings(policy: AndroidScreenshotPolicy, apply: (Boolean) -> Unit) {
    val scope = rememberCoroutineScope()
    var confirming by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var failedDisable by remember { mutableStateOf(false) }
    fun change(value: Boolean) {
        if (busy) return
        busy = true
        failed = false
        scope.launch {
            try {
                failed = !policy.setAllowed(value, apply)
                failedDisable = !value && failed
            } finally { busy = false }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        LabeledSwitch(tr("Разрешить скриншоты и запись экрана", "Allow screenshots and screen recording"),
            checked = policy.allowed, enabled = !busy,
            onChange = { if (it) confirming = true else change(false) })
        Text(tr("По умолчанию запрещены. Разрешение действует на открытые экраны приложения, включая заметки и документы.",
            "Blocked by default. Permission applies to open app screens, including notes and documents."),
            style = MaterialTheme.typography.bodySmall)
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        if (failed) {
            Text(if (failedDisable) tr(
                "Сохранить запрет не удалось. В этом запуске снимки запрещены; после перезапуска проверьте настройку.",
                "The restriction could not be saved. Capture is blocked for this session; check the setting after restarting.")
                else tr("Разрешение не сохранено. Снимки остаются запрещены.", "Permission was not saved. Capture remains blocked."),
                Modifier.announceChanges(), color = MaterialTheme.colorScheme.error)
            if (failedDisable) TextButton(enabled = !busy, onClick = { change(false) }) {
                Text(tr("Сохранить запрет", "Save restriction"))
            }
        }
    }
    if (confirming) AlertDialog(onDismissRequest = { confirming = false },
        title = { Text(tr("Разрешить снимки экрана?", "Allow screen capture?")) },
        text = { Text(tr("Снимок может содержать заметки, документы и ключи — он окажется вне защиты Paranoic.",
            "A capture may contain notes, documents and keys — it ends up outside Paranoic's protection.")) },
        confirmButton = { TextButton(enabled = !busy, onClick = { confirming = false; change(true) }) {
            Text(tr("Разрешить", "Allow"))
        } },
        dismissButton = { TextButton(onClick = { confirming = false }) { Text(tr("Оставить запрет", "Keep blocked")) } })
}
