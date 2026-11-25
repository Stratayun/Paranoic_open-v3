package io.github.stratayun.paranoic.ui.screens.settings

import io.github.stratayun.paranoic.ui.components.rememberPasswordAssessment
import io.github.stratayun.paranoic.ui.components.acceptsPassword
import io.github.stratayun.paranoic.ui.components.PeekToggle
import io.github.stratayun.paranoic.ui.components.peek
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
import io.github.stratayun.paranoic.ui.components.PasswordTools
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** Optional setup only. No password or completed security claim is persisted here. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun WelcomeFlow(core: CoreGateway, language: UiLanguage, epoch: Long, busy: Boolean, saving: Boolean, saved: Boolean?,
    onLanguage: (UiLanguage) -> Unit, onBusy: (Boolean) -> Unit, onFinish: () -> Unit,
    securitySettings: @Composable () -> Unit, vaultContent: @Composable () -> Unit,
    backHandler: @Composable (Boolean, () -> Unit) -> Unit) {
    var step by remember { mutableIntStateOf(0) }
    backHandler(step > 0) { if (!busy && !saving) step-- }
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(tr("Настройка Paranoic", "Set up Paranoic"), Modifier.sectionHeading(), style = MaterialTheme.typography.headlineSmall)
            Text(tr("Шаг ${step + 1} из 5", "Step ${step + 1} of 5") + " · " + when (step) {
                0 -> tr("Язык", "Language")
                1 -> tr("Блокировка", "App protection")
                2 -> tr("Пароль заметок", "Notes password")
                3 -> tr("Первое хранилище", "First vault")
                else -> tr("Ключ восстановления", "Recovery key")
            })
            if (saved == false) Text(tr("Настройки не удалось сохранить. Они действуют до перезапуска.", "Settings could not be saved. They remain active until restart."), color = MaterialTheme.colorScheme.error)
            if (step >= 3) Text(if (step == 3) tr("Создайте хранилище или откройте существующее. Этот шаг можно выполнить позже.", "Create a vault or open an existing one. You can do this later.")
                else tr("В открытом хранилище создайте ключ восстановления и сохраните его отдельно от устройства. Без пароля и заранее созданного ключа восстановить доступ нельзя.",
                    "Create a recovery key in the unlocked vault and keep it separately from the device. Access cannot be recovered without the password or a previously created key."))
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when (step) {
                    0 -> Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(tr("Выберите язык. Его можно изменить позже в настройках.", "Choose a language. Change it later in Settings."))
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) { UiLanguage.entries.forEach { choice ->
                            FilterChip(selected = language == choice, enabled = !saving, onClick = { onLanguage(choice) },
                                label = { Text(if (choice == UiLanguage.Russian) "Русский" else "English") })
                        } }
                    }
                    1 -> Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(tr("Системный вход защищает открытие приложения, если поддерживается платформой. Пароли заметок и хранилищ настраиваются отдельно. Уход в фон очищает открытые ключи.",
                            "System authentication protects app entry when supported by the platform. Notes and vault passwords are separate. Backgrounding clears open keys."))
                        securitySettings()
                    }
                    2 -> key(epoch) { WelcomeNotesPassword(core, onBusy) }
                    else -> vaultContent()
                }
            }
            if (saving) LinearProgressIndicator(Modifier.fillMaxWidth())
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(enabled = step > 0 && !busy && !saving, onClick = { step-- }) { Text(tr("Назад", "Back")) }
                Button(enabled = !busy && !saving, onClick = { if (step < 4) step++ else onFinish() }) {
                    Text(if (step < 4) tr("Далее / пропустить шаг", "Next / skip step") else tr("Завершить", "Finish"))
                }
                TextButton(enabled = !busy && !saving, onClick = onFinish) { Text(tr("Настроить позже", "Set up later")) }
            }
        }
    }
}

@Composable
private fun WelcomeNotesPassword(core: CoreGateway, onBusy: (Boolean) -> Unit) {
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<NotesStatus?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var revision by remember { mutableIntStateOf(0) }
    var busy by remember { mutableStateOf(false) }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    DisposableEffect(Unit) { onDispose { password = ""; repeat = ""; onBusy(false) } }
    LaunchedEffect(core, revision) {
        busy = true; status = null; fault = null
        try {
            when (val result = core.notesStatus()) {
                is CoreResult.Value -> status = result.value
                is CoreResult.Failure -> fault = result.fault
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { busy = false }
    }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(tr("Заметки защищены отдельно и не синхронизируются. Android удаляет их с приложением; на компьютере папка данных может сохраниться.", "Notes are separately protected and are not synced. Android removes them with the app; desktop app data may remain."))
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        fault?.let { FaultNotice(it) }
        if (status?.setUp == true) Text(tr("Пароль заметок уже настроен. Менять его здесь не требуется.", "A notes password is already configured. It does not need changing here."))
        if (status?.setUp == false) {
            OutlinedTextField(password, { if (it.length <= 4096) password = it }, enabled = !busy, singleLine = true,
                label = { Text(tr("Новый пароль", "New password")) }, visualTransformation = peek("WelcomeFlow:14"), trailingIcon = { PeekToggle("WelcomeFlow:14") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false))
            OutlinedTextField(repeat, { if (it.length <= 4096) repeat = it }, enabled = !busy, singleLine = true,
                label = { Text(tr("Повтор пароля", "Repeat password")) }, visualTransformation = peek("WelcomeFlow:15"), trailingIcon = { PeekToggle("WelcomeFlow:15") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false))
            val passwordAssessment = rememberPasswordAssessment(core, password)
            PasswordTools(core, password, !busy, assessment = passwordAssessment) { password = it; repeat = "" }
            Text(tr("Не менее 8 символов. Пароль не сохраняется в настройках мастера.", "At least 8 characters. The wizard never saves the password in preferences."))
            Button(enabled = !busy && passwordAssessment.acceptsPassword() && password == repeat && fault != CoreFault.OutcomeUnknown, onClick = {
                val secret = password; password = ""; repeat = ""; busy = true; onBusy(true)
                scope.launch {
                    try {
                        when (val result = core.notesCreate(secret)) {
                            is CoreResult.Value -> { core.lockSecrets(); revision++ }
                            is CoreResult.Failure -> { fault = result.fault; if (result.fault == CoreFault.OutcomeUnknown) status = null }
                        }
                    } catch (cancelled: CancellationException) { throw cancelled }
                    catch (_: Exception) { fault = CoreFault.OutcomeUnknown; status = null }
                    finally { busy = false; onBusy(false) }
                }
            }) { Text(tr("Создать пароль заметок", "Set notes password")) }
        }
        if (!busy && status == null) TextButton(onClick = { revision++ }) { Text(tr("Проверить состояние", "Check status")) }
    }
}
