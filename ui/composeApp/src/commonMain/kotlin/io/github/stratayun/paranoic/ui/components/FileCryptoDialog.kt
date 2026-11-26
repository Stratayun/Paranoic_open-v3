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
fun FileCryptoDialog(session: AppSession, source: FileItem, decrypt: Boolean, initialParent: Location? = null, picked: Boolean = false, onDismiss: () -> Unit) {
    val format = FileCryptoFormat.Age
    fun suggested(f: FileCryptoFormat): String = if (decrypt) source.name.removeSuffix("." + source.name.substringAfterLast('.'))
        .takeIf { it.isNotEmpty() && it != source.name } ?: source.name + ".decrypted" else source.name + "." + f.extension
    var parent by remember(source) { mutableStateOf(initialParent ?: session.files.folder) }
    // A taken name is offered as "name (1).ext" right away (owner 26.11: decrypting beside the kept original).
    var name by remember(source, decrypt) { mutableStateOf(suggested(format).let { wanted ->
        if (parent != session.files.folder) wanted else {
            val taken = session.files.entries.map { it.name.lowercase() }.toSet()
            val suffix = if (!decrypt && wanted.endsWith(".age")) ".age" else ""
            val stem = wanted.dropLast(suffix.length)
            val dot = stem.lastIndexOf('.').takeIf { it > 0 } ?: stem.length
            if (wanted.lowercase() !in taken) wanted
            else (1..999).map { stem.substring(0, dot) + " ($it)" + stem.substring(dot) + suffix }.firstOrNull { it.lowercase() !in taken } ?: wanted
        }
    }) }
    var password by remember { mutableStateOf("") }
    val passwordAssessment = rememberPasswordAssessment(session.core, if (decrypt) "" else password)
    var repeat by remember { mutableStateOf("") }
    var choosing by remember { mutableStateOf(false) }
    val destination = parent
    val collision = if (destination == session.files.folder) session.files.entries.any { it.name == name } else false
    val valid = destination != null && plausibleName(name) && !source.isTarget(destination, name) && !collision &&
        session.files.spaces.spaceOf(destination)?.writable == true && password.isNotEmpty() &&
        (decrypt || (passwordAssessment.acceptsPassword() && password == repeat)) && !session.writing &&
        session.directWrite?.phase != DirectWritePhase.Unknown && session.uncertain == null && session.uncertainAttention == null
    fun dismiss() { password = ""; repeat = ""; onDismiss() }
    AlertDialog(onDismissRequest = ::dismiss,
        title = { Text(if (decrypt) tr("Расшифровать файл", "Decrypt file") else tr("Зашифровать файл", "Encrypt file")) },
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(displayName(source.name))
                Text(if (decrypt) tr("Исходник не меняется.", "The source stays unchanged.")
                    else tr("Формат age — откроется age/rage на любом компьютере. Исходник остаётся; забытый пароль не восстановить.",
                        "age format — opens with age/rage anywhere. The source stays; a forgotten password cannot be recovered."),
                    style = MaterialTheme.typography.bodySmall)
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
                    PasswordTools(session.core, password, assessment = passwordAssessment) { password = it; repeat = "" }
                    OutlinedTextField(repeat, { repeat = it }, singleLine = true,
                        modifier = Modifier.fieldError(passwordMismatchText(), repeat.isNotEmpty() && password != repeat),
                        label = { Text(tr("Повторите пароль", "Repeat password")) }, visualTransformation = peek("FileCryptoDialog:3"), trailingIcon = { PeekToggle("FileCryptoDialog:3") },
                        isError = repeat.isNotEmpty() && password != repeat, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        supportingText = { if (repeat.isNotEmpty() && password != repeat) Text(passwordMismatchText()) })
                }
                if (decrypt) Text(tr("Результат будет незашифрованным и появится после полной проверки.", "The output is unencrypted and appears after a full check."),
                    style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { TextButton(enabled = valid, onClick = {
            val to = destination ?: return@TextButton
            val secret = password; password = ""; repeat = ""
            session.transformFile(source, to, name, format, decrypt, secret, picked)
            onDismiss()
        }) { Text(if (decrypt) tr("Расшифровать", "Decrypt") else tr("Зашифровать", "Encrypt")) } },
        dismissButton = { TextButton(onClick = ::dismiss) { Text(tr("Отмена", "Cancel")) } },
    )
    if (choosing) TransferDestinationDialog(session.core, source, session.files.spaces, TransferMode.Copy,
        onDismiss = { choosing = false }, onTransfer = { to, output, _, _ -> parent = to; name = output; choosing = false },
        conflictChoices = false, initialName = name, selectionOnly = true)
}

/**
 * age for several files with one password (owner 26.11). Each result goes beside its
 * source; existing names are never replaced; the first failure stops the rest.
 */
@Composable
fun BatchCryptoDialog(session: AppSession, sources: List<FileItem>, decrypt: Boolean, picked: Boolean = false, onDismiss: () -> Unit) {
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    val passwordAssessment = rememberPasswordAssessment(session.core, if (decrypt) "" else password)
    val files = sources.filter { it.kind == EntryKind.File }.take(128)
    val valid = files.isNotEmpty() && password.isNotEmpty() && (decrypt || (passwordAssessment.acceptsPassword() && password == repeat)) &&
        !session.writing && session.directWrite?.phase != DirectWritePhase.Unknown && session.uncertain == null && session.uncertainAttention == null
    fun dismiss() { password = ""; repeat = ""; onDismiss() }
    AlertDialog(onDismissRequest = ::dismiss,
        title = { Text(if (decrypt) tr("Расшифровать файлы: ${files.size}", "Decrypt ${files.size} files") else tr("Зашифровать файлы: ${files.size}", "Encrypt ${files.size} files")) },
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(files.take(4).joinToString("\n") { displayName(it.name) } + if (files.size > 4) "\n" + tr("и ещё ${files.size - 4}", "and ${files.size - 4} more") else "",
                    style = MaterialTheme.typography.bodySmall)
                Text(if (decrypt) tr("Каждый файл расшифруется рядом с собой. Исходники не меняются.", "Each file is decrypted beside itself. Sources stay unchanged.")
                    else tr("Рядом с каждым появится «имя.age». Исходники остаются; забытый пароль не восстановить.",
                        "Each gets a \"name.age\" beside it. Sources stay; a forgotten password cannot be recovered."))
                OutlinedTextField(password, { password = it }, singleLine = true,
                    label = { Text(tr("Пароль", "Password")) }, visualTransformation = peek("BatchCryptoDialog:1"), trailingIcon = { PeekToggle("BatchCryptoDialog:1") },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password))
                if (!decrypt) {
                    PasswordTools(session.core, password, assessment = passwordAssessment) { password = it; repeat = "" }
                    OutlinedTextField(repeat, { repeat = it }, singleLine = true,
                        modifier = Modifier.fieldError(passwordMismatchText(), repeat.isNotEmpty() && password != repeat),
                        label = { Text(tr("Повторите пароль", "Repeat password")) }, visualTransformation = peek("BatchCryptoDialog:2"), trailingIcon = { PeekToggle("BatchCryptoDialog:2") },
                        isError = repeat.isNotEmpty() && password != repeat, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password))
                }
            }
        },
        confirmButton = { TextButton(enabled = valid, onClick = {
            val secret = password; password = ""; repeat = ""
            session.transformFiles(files, decrypt, secret, picked)
            onDismiss()
        }) { Text(if (decrypt) tr("Расшифровать", "Decrypt") else tr("Зашифровать", "Encrypt")) } },
        dismissButton = { TextButton(onClick = ::dismiss) { Text(tr("Отмена", "Cancel")) } },
    )
}
