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
import io.github.stratayun.paranoic.ui.state.tr
import io.github.stratayun.paranoic.ui.state.fieldError

/** Preliminary UX check only; Rust enforces https (http only to this device). */
fun plausibleServerUrl(url: String): Boolean {
    val u = url.trim().lowercase()
    return u.startsWith("https://") && u.length > 8 ||
        u.startsWith("http://127.0.0.1") || u.startsWith("http://localhost")
}

/** Own server over WebDAV: address, login and app token. The token is passed to
 * Rust once, sealed there by the device key and never kept in UI state afterwards. */
@Composable
fun WebDavDialog(onDismiss: () -> Unit, onConnect: (url: String, user: String, token: String) -> Unit) {
    var url by remember { mutableStateOf("https://") }
    var user by remember { mutableStateOf("") }
    var token by remember { mutableStateOf("") }
    val valid = plausibleServerUrl(url) && user.isNotBlank() && token.isNotEmpty()
    val invalidUrl = url.length > 8 && !plausibleServerUrl(url)
    val urlError = tr("Нужен HTTPS-адрес; HTTP допустим только для этого устройства.", "Use an HTTPS address; HTTP is allowed only for this device.")
    AlertDialog(
        onDismissRequest = { token = ""; onDismiss() },
        title = { Text(tr("Подключить сервер (WebDAV)", "Connect server (WebDAV)")) },
        text = {
            Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(url, { if (it.length <= 2048) url = it }, Modifier.fillMaxWidth().fieldError(urlError, invalidUrl), singleLine = true,
                    label = { Text(tr("Адрес", "Address")) }, isError = invalidUrl, supportingText = { if (invalidUrl) Text(urlError) },
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Uri))
                OutlinedTextField(user, { if (it.length <= 256) user = it }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text(tr("Логин", "Login")) })
                OutlinedTextField(token, { if (it.length <= 4096) token = it }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text(tr("Токен или пароль приложения", "Token or app password")) },
                    visualTransformation = peek("WebDavDialog:1"), trailingIcon = { PeekToggle("WebDavDialog:1") }, keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password))
                Text(tr("HTTPS; HTTP допустим только для этого устройства. Токен хранится зашифрованным ключом устройства. Файлы на сервере открываются в файловом менеджере; защищённые заметки на сервер не попадают.",
                    "HTTPS; HTTP is allowed only for this device. The token is stored encrypted with the device key. Server files open in the file manager; protected notes never go to the server."))
            }
        },
        confirmButton = {
            TextButton(enabled = valid, onClick = { val t = token; token = ""; onConnect(url.trim(), user.trim(), t) }) { Text(tr("Подключить", "Connect")) }
        },
        dismissButton = { TextButton(onClick = { token = ""; onDismiss() }) { Text(tr("Отмена", "Cancel")) } },
    )
}
