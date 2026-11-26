package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** Reconciliation for vault screens. Reset secret UI models before allowing
 * another write; a successful fence alone must not re-enable an old notebook. */
@Composable
fun ProtectedWriteNotice(session: AppSession) {
    if (session.protectedWrite.phase != ProtectedWritePhase.Unknown) return
    var confirming by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    val scope = rememberCoroutineScope()
    Text(tr("Результат защищённой записи неизвестен. Новая запись заблокирована; проверьте хранилище и папки назначения.",
        "The protected write outcome is unknown. New writes are blocked; inspect the vault and destination folders."), color = MaterialTheme.colorScheme.error)
    TextButton(enabled = !busy, onClick = { confirming = true }) { Text(tr("Проверил(а), заблокировать и обновить…", "Inspected; lock and refresh…")) }
    fault?.let { FaultNotice(it) }
    if (confirming) {
        var checked by remember { mutableStateOf(false) }
        AlertDialog(onDismissRequest = { if (!busy) confirming = false }, title = { Text(tr("Завершить проверку состояния?", "Finish reconciliation?")) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Дождёмся текущей записи и закроем ключи. Несохранённые черновики пропадут.",
                    "We wait for the current write, then close the keys. Unsaved drafts are lost."))
                LabeledCheckbox(tr("Проверил(а) результат и разрешаю блокировку", "I inspected the result and agree to locking"), checked,
                    enabled = !busy, onChange = { checked = it })
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            } },
            confirmButton = { TextButton(enabled = checked && !busy, onClick = {
                busy = true; fault = null
                scope.launch {
                    try {
                        when (val result = session.core.awaitProtectedWrites()) {
                            is CoreResult.Value -> {
                                session.pause(); session.acknowledgeProtectedWrite(); session.resume(); confirming = false
                            }
                            is CoreResult.Failure -> fault = result.fault
                        }
                    } catch (cancelled: CancellationException) { throw cancelled }
                    catch (_: Exception) { fault = CoreFault.Unavailable }
                    finally { busy = false }
                }
            }) { Text(tr("Заблокировать и обновить", "Lock and refresh")) } },
            dismissButton = { TextButton(enabled = !busy, onClick = { confirming = false }) { Text(tr("Отмена", "Cancel")) } })
    }
}
