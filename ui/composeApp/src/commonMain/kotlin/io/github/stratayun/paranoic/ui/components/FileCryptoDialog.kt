package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun FileCryptoDialog(session: AppSession, source: FileItem, decrypt: Boolean, onDismiss: () -> Unit) {
    val format = FileCryptoFormat.Age
    fun suggested(f: FileCryptoFormat): String = if (decrypt) source.name.removeSuffix("." + source.name.substringAfterLast('.'))
        .takeIf { it.isNotEmpty() && it != source.name } ?: source.name + ".decrypted" else source.name + "." + f.extension
    var name by remember(source, decrypt) { mutableStateOf(suggested(format)) }
    var parent by remember(source) { mutableStateOf(session.files.folder) }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    var choosing by remember { mutableStateOf(false) }
    val destination = parent
    val collision = if (destination == session.files.folder) session.files.entries.any { it.name == name } else false
    val valid = destination != null && plausibleName(name) && !source.isTarget(destination, name) && !collision &&
        session.files.spaces.spaceOf(destination)?.writable == true && password.isNotEmpty() &&
        (decrypt || (PasswordRule.acceptable(password) && password == repeat)) && !session.writing &&
        session.directWrite?.phase != DirectWritePhase.Unknown && session.uncertain == null && session.uncertainAttention == null
    fun dismiss() { password = ""; repeat = ""; onDismiss() }
    AlertDialog(onDismissRequest = ::dismiss,
        title = { Text(if (decrypt) tr("Расшифровать файл", "Decrypt file") else tr("Зашифровать файл", "Encrypt file")) },
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(source.name))
                Text(tr("Формат age (ChaCha20-Poly1305, пароль через scrypt). Файл открывается программами age и rage на любом компьютере.",
                    "age format (ChaCha20-Poly1305, password via scrypt). The file opens with age and rage on any computer."), style = MaterialTheme.typography.bodySmall)
                Text(if (decrypt) tr("Расширение лишь подсказывает формат. Пароль может быть неверным, либо файл повреждён — эти случаи не всегда различимы.",
                    "The extension only suggests a format. A wrong password and a damaged file cannot always be distinguished.")
                    else tr("Новый файл в выбранной папке; исходник сохраняется. Забытый пароль восстановить нельзя.",
                        "A new file in the selected folder; the source is kept. A forgotten password cannot be recovered."))
                OutlinedTextField(name, { if (it.length <= 1024) name = it }, singleLine = true,
                    modifier = Modifier.fieldError(if (collision) tr("Имя уже занято.", "Name already exists.") else nameErrorText(), !plausibleName(name) || collision),
                    label = { Text(tr("Имя результата", "Output name")) }, isError = !plausibleName(name) || collision,
                    supportingText = { if (!plausibleName(name)) Text(nameErrorText()) })
                destination?.let { Text(tr("Назначение: ", "Destination: ") + displayName(session.files.spaces.spaceOf(it)?.name ?: it.storageId) + ": /" + displayName(it.path)) }
                TextButton(onClick = { choosing = true }) { Text(tr("Выбрать другую папку…", "Choose another folder…")) }
                if (collision) Text(tr("Это имя уже занято. Выберите другое.", "This name already exists. Choose another."), color = MaterialTheme.colorScheme.error)
                OutlinedTextField(password, { password = it }, singleLine = true,
                    label = { Text(tr("Пароль", "Password")) }, visualTransformation = peek("FileCryptoDialog:2"), trailingIcon = { PeekToggle("FileCryptoDialog:2") },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password))
                if (!decrypt) {
                    OutlinedTextField(repeat, { repeat = it }, singleLine = true,
                        modifier = Modifier.fieldError(passwordMismatchText(), repeat.isNotEmpty() && password != repeat),
                        label = { Text(tr("Повторите пароль", "Repeat password")) }, visualTransformation = peek("FileCryptoDialog:3"), trailingIcon = { PeekToggle("FileCryptoDialog:3") },
                        isError = repeat.isNotEmpty() && password != repeat, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        supportingText = { if (repeat.isNotEmpty() && password != repeat) Text(passwordMismatchText()) })
                    Text(tr("Минимум 12 символов; лучше длинная фраза.", "At least 12 characters; a long passphrase is better."))
                }
                Text(if (decrypt) tr("Результат будет незашифрованным. Он появится только после полной проверки файла, без перезаписи существующего имени.",
                    "The output will be unencrypted. It is published only after the whole file is verified, without overwriting an existing name.")
                    else tr("Существующие файлы не перезаписываются.", "Existing files are not overwritten."))
            }
        },
        confirmButton = { TextButton(enabled = valid, onClick = {
            val to = destination ?: return@TextButton
            val secret = password; password = ""; repeat = ""
            session.transformFile(source, to, name, format, decrypt, secret)
            onDismiss()
        }) { Text(if (decrypt) tr("Расшифровать", "Decrypt") else tr("Зашифровать", "Encrypt")) } },
        dismissButton = { TextButton(onClick = ::dismiss) { Text(tr("Отмена", "Cancel")) } },
    )
    if (choosing) TransferDestinationDialog(session.core, source, session.files.spaces, TransferMode.Copy,
        onDismiss = { choosing = false }, onTransfer = { to, output, _, _ -> parent = to; name = output; choosing = false },
        conflictChoices = false, initialName = name, selectionOnly = true)
}
