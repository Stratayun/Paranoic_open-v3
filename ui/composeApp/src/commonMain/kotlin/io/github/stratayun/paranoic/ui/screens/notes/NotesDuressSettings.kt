package io.github.stratayun.paranoic.ui.screens.notes

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

@Composable
internal fun NotesDuressSettings(session: AppSession, enabled: Boolean, onSet: (String?) -> Unit) {
    var configured by remember { mutableStateOf<Boolean?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var setting by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf(false) }
    LaunchedEffect(session.core, session.protectedWrite.ticket, enabled, attempt) {
        configured = null; fault = null
        if (!enabled) return@LaunchedEffect
        try {
            val result = session.core.notesDuressEnabled()
            currentCoroutineContext().ensureActive()
            when (result) {
                is CoreResult.Value -> configured = result.value
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
    }
    Text(tr("Пароль под принуждением", "Duress password"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
    Text(when (configured) {
        true -> tr("Настроен", "Configured")
        false -> tr("Выключен", "Off")
        null -> tr("Состояние не прочитано", "State has not been read")
    })
    Text(tr("Вход с этим паролем необратимо стирает прежние заметки и личную папку, затем открывает пустые заметки. При входе отдельного сообщения не будет. Внешние копии не стираются.",
        "Unlocking with this password irreversibly wipes the previous notes and personal folder, then opens empty notes. Unlocking shows no special message. External copies are not erased."),
        style = MaterialTheme.typography.bodySmall)
    fault?.let { FaultNotice(it) }
    Row {
        TextButton(enabled = enabled && configured != null, onClick = { setting = true }) { Text(tr("Настроить / заменить…", "Configure / replace…")) }
        TextButton(enabled = enabled && configured == true, onClick = { removing = true }) { Text(tr("Выключить…", "Turn off…")) }
        if (fault != null) TextButton(enabled = enabled, onClick = { attempt++ }) { Text(tr("Перечитать", "Reload")) }
    }
    if (setting) {
        var password by remember { mutableStateOf("") }
        var repeated by remember { mutableStateOf("") }
        var acknowledged by remember { mutableStateOf(false) }
        var strength by remember { mutableStateOf<PasswordStrength?>(null) }
        DisposableEffect(Unit) { onDispose { password = ""; repeated = ""; strength = null } }
        val ready = enabled && password.isNotEmpty() && password == repeated && acknowledged &&
            strength != null && strength != PasswordStrength.Weak
        AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { password = ""; repeated = ""; setting = false },
            title = { Text(tr("Настроить пароль под принуждением", "Configure a duress password")) },
            text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Пароль должен быть сильным и отличаться от основного. Не проверяйте его вводом на экране входа: это уничтожит прежние данные.",
                    "Use a strong password different from the main password. Do not test it on the unlock screen: that would destroy the previous data."),
                    color = MaterialTheme.colorScheme.error)
                OutlinedTextField(password, { if (it.length <= 4096) { password = it; strength = null } }, singleLine = true,
                    label = { Text(tr("Пароль под принуждением", "Duress password")) }, visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                val assessed = PasswordTools(session.core, password, enabled) { password = it; repeated = ""; strength = null }
                SideEffect { strength = assessed }
                OutlinedTextField(repeated, { if (it.length <= 4096) repeated = it }, singleLine = true,
                    label = { Text(tr("Повторите пароль", "Repeat password")) }, visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
                    isError = repeated.isNotEmpty() && repeated != password)
                LabeledCheckbox(tr("Понимаю: использование этого пароля уничтожит прежние данные", "I understand: using this password destroys the previous data"),
                    acknowledged, onChange = { acknowledged = it })
            } },
            confirmButton = { TextButton(enabled = ready, onClick = {
                if (ready) { val secret = password; password = ""; repeated = ""; setting = false; onSet(secret) }
            }) { Text(tr("Настроить", "Configure")) } },
            dismissButton = { TextButton(onClick = { password = ""; repeated = ""; setting = false }) { Text(tr("Отмена", "Cancel")) } })
    }
    if (removing) AlertDialog(onDismissRequest = { removing = false }, title = { Text(tr("Выключить пароль под принуждением?", "Turn off the duress password?")) },
        text = { Text(tr("Данные не стираются. Дополнительный пароль перестанет действовать в текущем Сейфе; внешние старые копии не изменятся.",
            "Data is kept. The additional password stops working in the current Safe; older external copies remain unchanged.")) },
        confirmButton = { TextButton(enabled = enabled, onClick = { removing = false; onSet(null) }) { Text(tr("Выключить", "Turn off")) } },
        dismissButton = { TextButton(onClick = { removing = false }) { Text(tr("Отмена", "Cancel")) } })
}
