package io.github.stratayun.paranoic.ui.screens.encryption

import io.github.stratayun.paranoic.ui.components.rememberPasswordAssessment
import io.github.stratayun.paranoic.ui.components.acceptsPassword
import io.github.stratayun.paranoic.ui.components.PeekToggle
import io.github.stratayun.paranoic.ui.components.peek
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.components.LabeledCheckbox
import io.github.stratayun.paranoic.ui.components.PasswordTools
import io.github.stratayun.paranoic.bridge.CoreGateway

@Composable
internal fun VaultChangePasswordDialog(core: CoreGateway, name: String, cryptomator: Boolean, onClose: () -> Unit,
    onConfirm: (String, String) -> Unit) {
    var old by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    val passwordAssessment = rememberPasswordAssessment(core, password)
    var repeat by remember { mutableStateOf("") }
    fun clear() { old = ""; password = ""; repeat = "" }
    DisposableEffect(Unit) { onDispose { clear() } }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { clear(); onClose() }, title = { Text(tr("Сменить пароль папки", "Change vault password")) },
        text = {
            Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(name))
                SecretField(old, { if (it.length <= 1024) old = it }, tr("Текущий пароль", "Current password"))
                SecretField(password, { if (it.length <= 1024) password = it }, tr("Новый пароль", "New password"))
                PasswordTools(core, password, assessment = passwordAssessment) { password = it; repeat = "" }
                SecretField(repeat, { if (it.length <= 1024) repeat = it }, tr("Повторите новый пароль", "Repeat new password"),
                    if (repeat.isNotEmpty() && repeat != password) passwordMismatchText() else null)
                Text(tr("Новый пароль: от 12 символов; лучше длинная фраза. Текущий пароль проверяется перед изменением.",
                    "Use 12 characters or more, preferably a long phrase. The current password is verified before changing it."))
                Text(tr("Старые копии заголовка в облаке и бэкапах по-прежнему открываются старым паролем.",
                    "Old header copies in clouds and backups still open with the old password."))
                if (cryptomator) Text(tr("Прерванная смена пароля доводится при следующем открытии Cryptomator. После неё нужен новый пароль.",
                    "An interrupted Cryptomator password change is completed on the next open. Use the new password afterwards."))
            }
        },
        confirmButton = { TextButton(enabled = old.isNotEmpty() && passwordAssessment.acceptsPassword() && password == repeat && password != old,
            onClick = {
                if (old.isNotEmpty() && passwordAssessment.acceptsPassword() && password == repeat && password != old) {
                    val o = old; val n = password; clear(); onConfirm(o, n)
                }
            }) { Text(tr("Сменить", "Change")) } },
        dismissButton = { TextButton(onClick = { clear(); onClose() }) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
internal fun VaultRecoverySetupDialog(name: String, onClose: () -> Unit, onConfirm: (String) -> Unit) {
    var password by remember { mutableStateOf("") }
    var confirmed by remember { mutableStateOf(false) }
    DisposableEffect(Unit) { onDispose { password = "" } }
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { password = ""; onClose() }, title = { Text(tr("Новый ключ восстановления", "New recovery key")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(displayName(name))
            Text(tr("Ключ откроет папку без пароля и заменит прежний. Старые копии заголовка могут хранить прежний доступ.",
                "The key opens the folder without a password and replaces the old one. Old header copies may keep the old access."))
            Text(tr("Ключ будет показан один раз — запишите его вне приложения.",
                "The key is shown once — write it down outside the app."))
            SecretField(password, { if (it.length <= 1024) password = it }, tr("Текущий пароль", "Current password"))
            LabeledCheckbox(tr("Понимаю: прежний ключ будет заменён", "I understand: the previous key will be replaced"),
                checked = confirmed, onChange = { confirmed = it })
        } },
        confirmButton = { TextButton(enabled = confirmed && password.isNotEmpty(), onClick = {
            if (confirmed && password.isNotEmpty()) { val p = password; password = ""; onConfirm(p) }
        }) { Text(tr("Создать ключ", "Create key")) } },
        dismissButton = { TextButton(onClick = { password = ""; onClose() }) { Text(tr("Отмена", "Cancel")) } })
}


@Composable
internal fun VaultRecoveryOpenDialog(core: CoreGateway, name: String, canReset: Boolean, onClose: () -> Unit,
    onConfirm: (recovery: String, newPassword: String?) -> Unit) {
    var recovery by remember { mutableStateOf("") }
    var resetting by remember { mutableStateOf(false) }
    var password by remember { mutableStateOf("") }
    val passwordAssessment = rememberPasswordAssessment(core, password)
    var repeated by remember { mutableStateOf("") }
    fun clear() { recovery = ""; password = ""; repeated = "" }
    DisposableEffect(Unit) { onDispose { clear() } }
    val compact = recovery.filterNot { it in "- \t\n\r" }.uppercase()
    val valid = compact.length == 52 && compact.all { it in 'A'..'Z' || it in '2'..'7' } &&
        (!resetting || canReset && passwordAssessment.acceptsPassword() && password == repeated)
    AlertDialog(modifier = Modifier.trackSecretActivity(), onDismissRequest = { clear(); onClose() },
        title = { Text(if (resetting) tr("Новый пароль по ключу", "Reset password with a key") else tr("Открыть по ключу восстановления", "Open with recovery key")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(displayName(name))
            Text(if (resetting) tr("Введите заранее созданный ключ: без него забытый пароль сбросить нельзя.",
                "Enter a previously created key: a forgotten password cannot be reset without it.")
                else tr("Введите ключ, который был заранее создан для этой папки. Пробелы и дефисы допустимы. Пароль при этом не меняется.",
                    "Enter the key previously created for this vault. Spaces and dashes are allowed. The password is not changed."))
            SecretField(recovery, { if (it.length <= 256) recovery = it }, tr("Ключ восстановления", "Recovery key"))
            LabeledCheckbox(tr("Забыли пароль? Задать новый по ключу", "Forgot your password? Reset it with the key"),
                checked = resetting, enabled = canReset, onChange = { resetting = it; password = ""; repeated = "" })
            if (!canReset) Text(tr("Изменение пароля недоступно в режиме чтения или пока настройки хранилища не прочитаны.",
                "Password reset is unavailable in read-only mode or until vault preferences are loaded."))
            if (resetting) {
                SecretField(password, { if (it.length <= 4096) password = it }, tr("Новый пароль", "New password"))
                PasswordTools(core, password, assessment = passwordAssessment) { password = it; repeated = "" }
                SecretField(repeated, { if (it.length <= 4096) repeated = it }, tr("Повторите новый пароль", "Repeat new password"),
                    if (repeated.isNotEmpty() && repeated != password) passwordMismatchText() else null)
                Text(tr("От 12 символов. Старые копии заголовка могут открываться старым паролем.",
                    "12+ characters. Old header copies may still open with the old password."))
            }
            Text(tr("Ключ хранилища не восстанавливает заметки. Заметки не переносятся и удаляются вместе с приложением.",
                "A vault key cannot recover notes. Notes are not transferred and are removed with the app."), style = MaterialTheme.typography.bodySmall)
        } },
        confirmButton = { TextButton(enabled = valid, onClick = {
            val r = recovery; val next = password.takeIf { resetting }; clear(); onConfirm(r, next)
        }) { Text(if (resetting) tr("Задать пароль и открыть", "Reset password and open") else tr("Открыть", "Open")) } },
        dismissButton = { TextButton(onClick = { clear(); onClose() }) { Text(tr("Отмена", "Cancel")) } })
}

@Composable
private fun SecretField(value: String, onChange: (String) -> Unit, label: String, error: String? = null) {
    OutlinedTextField(value = value, onValueChange = onChange, modifier = Modifier.fillMaxWidth().fieldError(error.orEmpty(), error != null), singleLine = true,
        // Audit A44: each field has its own eye; the label is unique within a dialog.
        label = { Text(label) }, visualTransformation = peek("VaultSecurityDialogs:" + label), trailingIcon = { PeekToggle("VaultSecurityDialogs:" + label) },
        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
        isError = error != null, supportingText = { error?.let { Text(it) } })
}
