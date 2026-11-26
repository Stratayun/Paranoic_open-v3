package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import io.github.stratayun.paranoic.bridge.CoreFault
import io.github.stratayun.paranoic.bridge.JobError
import io.github.stratayun.paranoic.bridge.JobKind
import io.github.stratayun.paranoic.bridge.JobPhase
import io.github.stratayun.paranoic.ui.navigation.Destination

val LocalUiLanguage = staticCompositionLocalOf { UiLanguage.Russian }
@Composable
fun tr(ru: String, en: String): String = if (LocalUiLanguage.current == UiLanguage.Russian) ru else en

@Composable
fun nameErrorText() = tr("Недопустимое имя. Выберите другое имя без разделителей пути.",
    "Invalid name. Choose another name without path separators.")
@Composable
fun passwordMismatchText() = tr("Пароли не совпадают.", "Passwords do not match.")

@Composable
fun Destination.label(): String = when (this) {
    Destination.Files -> tr("Файлы", "Files")
    Destination.Encryption -> tr("Крипто-папки", "Crypto folders")
    Destination.Notes -> tr("Сейф", "Safe")
    Destination.Sync -> tr("Синхронизация и бэкап", "Sync and backup")
    Destination.Operations -> tr("Операции", "Operations")
    Destination.Settings -> tr("Настройки", "Settings")
    Destination.Diagnostics -> tr("Диагностика", "Diagnostics")
}
@Composable
fun CoreFault.label(): String = when (this) {
    CoreFault.Unsupported -> tr("Формат, движок или возможность хранилища недоступны.", "Format, engine or storage capability is unavailable.")
    CoreFault.Limit -> tr("Превышен лимит размера или сложности.", "Size or complexity limit exceeded.")
    CoreFault.Changed -> tr("Файл изменился во время чтения. Откройте его заново.", "The file changed while reading. Reopen it.")
    CoreFault.Disconnected -> tr("Rust Core ещё не подключён. Файловые действия недоступны.", "Rust Core is not connected yet. File actions are unavailable.")
    CoreFault.Permission -> tr("Нет доступа. Проверьте разрешение на папку.", "Access denied. Check the folder permission.")
    CoreFault.Unavailable -> tr("Данные недоступны.", "Data unavailable.")
    CoreFault.Cancelled -> tr("Операция отменена.", "Operation cancelled.")
    CoreFault.Conflict -> tr("Конфликт. Существующие файлы не перезаписаны.", "Conflict. Existing files were not overwritten.")
    CoreFault.InvalidName -> tr("Недопустимое имя.", "Invalid name.")
    CoreFault.Busy -> tr("Папка используется незавершённым заданием. Дождитесь его окончания.", "The folder is used by an unfinished job. Wait until it ends.")
    CoreFault.PasswordRequired -> tr("Архив зашифрован. Введите пароль.", "The archive is encrypted. Enter its password.")
    CoreFault.WrongPassword -> tr("Неверный пароль.", "Wrong password.")
    CoreFault.WeakPassword -> tr("Пароль слишком слабый: нужно от 12 символов, без распространённых паролей и повторов.",
        "The password is too weak: use 12+ characters, no common passwords or repeats.")
    CoreFault.RateLimited -> tr("Слишком много попыток. Подождите и попробуйте снова.", "Too many attempts. Wait and try again.")
    CoreFault.Wiped -> tr("Заметки стёрты после серии неверных паролей. Раздел можно создать заново.",
        "Notes were wiped after repeated wrong passwords. The section can be set up again.")
    CoreFault.NotSetUp -> tr("Заметки ещё не созданы.", "Notes are not set up yet.")
    CoreFault.Locked -> tr("Заблокировано. Введите пароль снова.", "Locked. Enter the password again.")
    CoreFault.DeviceKey -> tr("Ключ устройства недоступен: защищённое хранение на этом устройстве не работает.",
        "The device key is unavailable: protected storage does not work on this device.")
    CoreFault.OutcomeUnknown -> tr("Результат запроса неизвестен. Проверьте журнал; не повторяйте запись автоматически.", "Request outcome is unknown. Inspect the journal; do not automatically repeat the write.")
}
@Composable
fun JobPhase.label(): String = when (this) {
    JobPhase.Queued -> tr("В очереди", "Queued")
    JobPhase.Running -> tr("Выполняется", "Running")
    JobPhase.Succeeded -> tr("Завершено", "Completed")
    JobPhase.Failed -> tr("Ошибка", "Failed")
    JobPhase.Cancelled -> tr("Отменено", "Cancelled")
    JobPhase.NeedsAttention -> tr("Требуется внимание", "Needs attention")
}
@Composable
fun JobKind.label(): String = when (this) {
    JobKind.CreateFolder -> tr("Создание папки", "Create folder")
    JobKind.CreateFile -> tr("Создание файла", "Create file")
    JobKind.Copy -> tr("Копирование", "Copy")
    JobKind.Move -> tr("Перемещение", "Move")
    JobKind.Delete -> tr("Удаление", "Delete")
    JobKind.Save -> tr("Сохранение", "Save")
    JobKind.Recovery -> tr("Восстановление", "Recovery")
    JobKind.Rename -> tr("Переименование", "Rename")
    JobKind.Other -> tr("Операция", "Operation")
}
@Composable
fun JobError.label(): String = when (this) {
    JobError.Permission -> tr("нет доступа", "access denied")
    JobError.Exists -> tr("имя уже занято, ничего не перезаписано", "name already taken, nothing overwritten")
    JobError.NotFound -> tr("исходный файл или папка не найдены", "source or folder not found")
    JobError.Changed -> tr("файл изменился во время операции", "the file changed during the operation")
    JobError.Limit -> tr("превышен лимит", "limit exceeded")
    JobError.Unsupported -> tr("хранилище не поддерживает операцию", "the storage does not support the operation")
    JobError.Cancelled -> tr("отменено", "cancelled")
    JobError.Attention -> tr("результат требует проверки", "the result needs checking")
    JobError.Invalid -> tr("недопустимый запрос или имя", "invalid request or name")
    JobError.Io -> tr("ошибка ввода-вывода", "input/output error")
    JobError.Other -> tr("внутренняя ошибка", "internal error")
}
/** Presentation only. Never use this value as a path or command argument. */
fun displayName(value: String): String = value.map {
    if (it.isISOControl() || it in '\u202A'..'\u202E' || it in '\u2066'..'\u2069') '\uFFFD' else it
}.joinToString("")
