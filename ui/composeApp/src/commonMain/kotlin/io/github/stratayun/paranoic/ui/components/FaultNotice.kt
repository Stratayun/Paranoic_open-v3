package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException

val LocalErrorCore = staticCompositionLocalOf<CoreGateway> { DisconnectedCoreGateway }

@Composable
fun FaultNotice(fault: CoreFault, modifier: Modifier = Modifier, message: String? = null) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(message ?: fault.label(), Modifier.announceChanges(), color = MaterialTheme.colorScheme.error)
        FaultHelp(fault)
    }
}

/** Recent diagnostics are shared events, never falsely attributed to this request. */
@Composable
fun FaultHelp(fault: CoreFault, code: String = fault.name) {
    val core = LocalErrorCore.current
    var show by remember(fault, code) { mutableStateOf(false) }
    var recent by remember { mutableStateOf<List<Diagnostic>>(emptyList()) }
    var loading by remember { mutableStateOf(false) }
    var unavailable by remember { mutableStateOf(false) }
    var fullJournal by remember { mutableStateOf(false) }
    TextButton(onClick = { show = true }) { Text(tr("Подробнее", "Details")) }
    if (show) {
        LaunchedEffect(core, fault, code) {
            loading = true; recent = emptyList(); unavailable = false
            try {
                when (val result = core.diagnostics()) {
                    is CoreResult.Value -> recent = result.value.sortedByDescending { it.atMs }.take(100)
                    is CoreResult.Failure -> unavailable = true
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { unavailable = true }
            finally { loading = false }
        }
        AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { show = false }, title = { Text(tr("Сведения об ошибке", "Error details")) },
            text = { Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Код: ", "Code: ") + code)
                Text(fault.guidance())
                if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
                if (recent.isNotEmpty()) Text(tr("Последние сообщения ядра — общий журнал, не привязанный к одному запросу:",
                    "Recent Core messages from the shared journal, not tied to a single request:"))
                recent.take(if (fullJournal) 100 else 5).forEach { Text(it.code + ": " + it.message) }
                if (unavailable) Text(tr("Журнал сейчас недоступен. Код и подсказка доступны без него.", "The journal is unavailable. The code and guidance remain available."))
            } }, confirmButton = { TextButton(onClick = { show = false }) { Text(tr("Закрыть", "Close")) } },
            dismissButton = { TextButton(onClick = { fullJournal = !fullJournal }) { Text(if (fullJournal) tr("Кратко", "Summary") else tr("Весь журнал", "Full journal")) } })
    }
}

@Composable
fun JobFaultNotice(error: JobError) {
    val fault = when (error) {
        JobError.Permission -> CoreFault.Permission
        JobError.Exists -> CoreFault.Conflict
        JobError.NotFound, JobError.Changed -> CoreFault.Changed
        JobError.Limit -> CoreFault.Limit
        JobError.Unsupported -> CoreFault.Unsupported
        JobError.Cancelled -> CoreFault.Cancelled
        JobError.Attention -> CoreFault.OutcomeUnknown
        JobError.Invalid -> CoreFault.InvalidName
        JobError.Io, JobError.Other -> CoreFault.Unavailable
    }
    Column { Text(error.label(), color = MaterialTheme.colorScheme.error); FaultHelp(fault, error.name) }
}

@Composable
private fun CoreFault.guidance(): String = when (this) {
    CoreFault.Disconnected -> tr("Ядро не запущено. Вернитесь к сообщению запуска и повторите запуск после устранения причины.", "Core is not running. Return to the startup message and retry after resolving its cause.")
    CoreFault.Permission -> tr("Проверьте права выбранной папки и режим чтения хранилища. Повторное подключение папки выполняется через «Места».", "Check the selected folder's permissions and the vault's read-only mode. Reconnect folders through Places.")
    CoreFault.Unavailable -> tr("Проверьте устройство или подключение. Подробности — в журнале.", "Check the device or connection. Details are in the journal.")
    CoreFault.Conflict -> tr("Имя уже занято. Выберите другое имя или сохранение обоих объектов, если эта операция его поддерживает.", "The name exists. Choose another name or Keep both if the operation supports it.")
    CoreFault.InvalidName -> tr("Проверьте имя и обязательные поля. В имени нельзя использовать разделители пути, служебные символы, «.» или «..».", "Check the name and required fields. Names cannot contain path separators, control characters, dot or dot-dot.")
    CoreFault.OutcomeUnknown -> tr("Проверьте журнал, задания и папки до нового запуска. Потерянный ответ не означает, что запись не состоялась.", "Inspect the journal, jobs and folders before starting again. A lost reply does not mean the write did not happen.")
    CoreFault.Unsupported -> tr("Для этого формата или места операция недоступна.", "Not available for this format or place.")
    CoreFault.Limit -> tr("Уменьшите размер или количество объектов. Проверьте ограничение, указанное на текущем экране.", "Reduce the size or number of entries. Check the limit shown on the current screen.")
    CoreFault.Changed -> tr("Перечитайте папку или документ и повторите действие.", "Reload the folder or document and try again.")
    CoreFault.Busy -> tr("Дождитесь завершения текущей операции.", "Wait for the current operation to finish.")
    CoreFault.PasswordRequired -> tr("Откройте «Формат / пароль» и введите пароль архива. Пароль не сохраняется; при распаковке его нужно ввести снова.", "Open Format / password and enter the archive password. It is not stored; enter it again when extracting.")
    CoreFault.WrongPassword -> tr("Проверьте пароль и раскладку. У 7z это может значить и повреждение архива.", "Check the password and keyboard layout. For 7z it may also mean a damaged archive.")
    CoreFault.WeakPassword -> tr("Используйте более длинный уникальный пароль или фразу из нескольких случайных слов.", "Use a longer unique password or a phrase of several random words.")
    CoreFault.RateLimited -> tr("Выдержите паузу перед следующей попыткой. Автоматический подбор и повтор не выполняются.", "Wait before trying again. There are no automatic password attempts or retries.")
    CoreFault.Wiped -> tr("Защищённые данные были очищены. Создайте новое пустое защищённое пространство; отменить уничтожение ключей нельзя.", "Protected data was cleared. Set up a new empty protected space; key destruction cannot be undone.")
    CoreFault.NotSetUp -> tr("Сначала создайте защищённое пространство в соответствующем разделе.", "Set up the protected space in its section first.")
    CoreFault.Locked -> tr("Откройте защищённый раздел заново. После блокировки старые ключи и сеансы не восстанавливаются автоматически.", "Unlock the protected section again. Old keys and sessions are not restored automatically after locking.")
    CoreFault.DeviceKey -> tr("Проверьте системный вход и хранилище ключей устройства. Данные не сбрасываются.", "Check system sign-in and the device key store. No data is reset.")
    CoreFault.Cancelled -> tr("Запрос отменён. Проверьте задания: отмена ожидания или отправки не гарантирует отмену уже принятой записи.", "The request was cancelled. Check jobs: cancelling a wait or submission does not guarantee an accepted write was cancelled.")
}
