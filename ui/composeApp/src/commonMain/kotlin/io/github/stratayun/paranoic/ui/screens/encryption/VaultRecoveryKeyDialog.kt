package io.github.stratayun.paranoic.ui.screens.encryption

import io.github.stratayun.paranoic.ui.components.PeekToggle
import io.github.stratayun.paranoic.ui.components.peek
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch

/** The key was already published. Only successful Core verification closes
 * this dialog; backgrounding/Quick Lock still revokes the entire secret screen. */
@Composable
internal fun VaultRecoveryKeyDialog(core: CoreGateway, keyText: String, onClose: () -> Unit) {
    var indices by remember(keyText) { mutableStateOf<List<Int>>(emptyList()) }
    var answers by remember(keyText) { mutableStateOf<List<String>>(emptyList()) }
    var checking by remember(keyText) { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var mismatch by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(core, keyText, attempt) {
        fault = null; indices = emptyList(); answers = emptyList()
        try {
            val result = core.vaultRecoveryChallenge(3)
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> {
                    if (result.value.size != 3 || result.value.distinct().size != 3 || result.value.any { it < 0 })
                        fault = CoreFault.Unavailable
                    else { indices = result.value; answers = List(3) { "" } }
                }
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
    }
    DisposableEffect(keyText) { onDispose { answers = emptyList(); indices = emptyList() } }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = {},
        properties = DialogProperties(dismissOnBackPress = false, dismissOnClickOutside = false),
        title = { Text(if (checking) tr("Проверьте запись ключа", "Verify the recorded key") else tr("Запишите ключ сейчас", "Record the key now")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(tr("Этот ключ открывает хранилище без пароля. Храните его отдельно от устройства и никому не передавайте. После закрытия или блокировки приложение не покажет его повторно.",
                "This key opens the vault without a password. Keep it separately from this device and do not share it. After closing or locking, the app cannot show it again."))
            if (!checking) {
                // No selection, clipboard, sharing, live-region or saveable state.
                Text(keyText, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.titleMedium)
                Text(tr("Чтобы закрыть окно, потребуется ввести три случайные группы со своей записи.",
                    "To close this window, you will need to enter three random groups from your recorded copy."))
            } else {
                indices.forEachIndexed { position, group ->
                    OutlinedTextField(answers.getOrElse(position) { "" }, { value ->
                        if (value.length <= 32 && !busy) {
                            answers = answers.mapIndexed { i, previous -> if (i == position) value else previous }
                            mismatch = false
                        }
                    }, singleLine = true, enabled = !busy,
                        label = { Text(tr("Группа ${group.toLong() + 1}", "Group ${group.toLong() + 1}")) },
                        visualTransformation = peek("VaultRecoveryKeyDialog:19"), trailingIcon = { PeekToggle("VaultRecoveryKeyDialog:19") },
                        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                }
                if (mismatch) Text(tr("Группы не совпали. Проверьте свою запись.", "The groups did not match. Check your recorded copy."),
                    color = MaterialTheme.colorScheme.error)
                TextButton(enabled = !busy, onClick = { answers = List(indices.size) { "" }; checking = false; mismatch = false }) {
                    Text(tr("Показать ключ ещё раз", "Show the key again"))
                }
            }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it) }
            if (indices.isEmpty() && fault != null) TextButton(onClick = { attempt++ }) { Text(tr("Повторить запрос групп", "Request groups again")) }
        } },
        confirmButton = { TextButton(enabled = !busy && indices.size == 3 && (!checking || answers.size == 3 && answers.all { it.isNotBlank() }),
            onClick = {
                if (!checking) { checking = true; mismatch = false; return@TextButton }
                val submitted = answers.toList()
                answers = List(indices.size) { "" }; busy = true; fault = null; mismatch = false
                scope.launch {
                    try {
                        val result = core.vaultRecoveryCheck(keyText, indices, submitted)
                        currentCoroutineContext().ensureActive()
                        when (result) {
                            is CoreResult.Value -> if (result.value) onClose() else mismatch = true
                            is CoreResult.Failure -> fault = result.fault
                        }
                    } catch (cancelled: CancellationException) { throw cancelled }
                    catch (_: Exception) { fault = CoreFault.Unavailable }
                    finally { busy = false }
                }
            }) { Text(if (checking) tr("Проверить и закрыть", "Verify and close") else tr("Записал(а), проверить", "Recorded, verify")) } })
}
