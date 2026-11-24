package io.github.stratayun.paranoic.ui.screens.operations

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.LabeledCheckbox
import io.github.stratayun.paranoic.ui.components.FaultHelp
import io.github.stratayun.paranoic.ui.state.*

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AttentionCard(session: AppSession, item: AttentionItem, onOpenFolder: (Location) -> Unit) {
    var details by remember(item.job.id, session.secretsEpoch) { mutableStateOf(false) }
    var decision by remember(item, session.secretsEpoch) { mutableStateOf<AttentionAction?>(null) }
    val pending = session.attentionPending(item.job.id)
    val enabled = session.core.connected && !session.writing && !session.attention.loading &&
        session.attention.fault == null && session.uncertain == null && session.uncertainAttention == null && !pending
    Surface(shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(item.job.kind.label() + " · " + tr("Требуется решение", "Decision needed"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
            Text(item.reason.explanation())
            FaultHelp(CoreFault.OutcomeUnknown, item.job.error?.name ?: item.reason.name)
            Text(tr("Задание: ", "Job: ") + displayName(item.job.id))
            item.job.source?.let { location ->
                Text(tr("Исходный объект: ", "Source entry: ") + location.description(session.files.spaces))
            }
            item.job.target?.let { location ->
                Text(tr("Назначение / затронутый объект: ", "Destination / affected entry: ") + location.description(session.files.spaces))
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                item.job.source?.let { location ->
                    TextButton(onClick = { onOpenFolder(location) }, enabled = session.files.spaces.spaceOf(location) != null) {
                        Text(tr("Открыть исходную папку", "Open source folder"))
                    }
                }
                item.job.target?.let { location ->
                    TextButton(onClick = { onOpenFolder(location) }, enabled = session.files.spaces.spaceOf(location) != null) {
                        Text(tr("Открыть папку назначения", "Open destination folder"))
                    }
                }
            }
            if (listOfNotNull(item.job.source, item.job.target).any { session.files.spaces.spaceOf(it) == null })
                Text(tr("Для осмотра недоступной папки сначала подключите её пространство.",
                    "Connect the missing space before inspecting its folder."))
            if (pending) Text(tr("Решение уже отправлено. Дождитесь результата задания или проверьте потерянный ответ по ID.",
                "A decision has already been submitted. Wait for the job result or check the lost reply by ID."))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (item.canRestore) TextButton(onClick = { decision = AttentionAction.Restore }, enabled = enabled) {
                    Text(tr("Вернуть прежнюю версию…", "Restore previous version…"))
                }
                if (item.canDiscard) TextButton(onClick = { decision = AttentionAction.Discard }, enabled = enabled) {
                    Text(tr("Удалить резервную копию…", "Delete backup…"))
                }
                if (item.canAccept) TextButton(onClick = { decision = AttentionAction.Accept }, enabled = enabled) {
                    Text(tr("Проверить и принять результат…", "Verify and accept result…"))
                }
                TextButton(onClick = { decision = AttentionAction.Reviewed }, enabled = enabled) {
                    Text(tr("Разобрано…", "Mark reviewed…"))
                }
            }
            TextButton(onClick = { details = !details }) { Text(if (details) tr("Скрыть подробности", "Hide details")
                else tr("Подробнее", "Details")) }
            if (details) {
                item.backup?.let { Text(tr("Резервная копия: ", "Backup: ") + it.description(session.files.spaces)) }
                Text(if (item.detail.isBlank()) tr("Дополнительных сведений нет.", "No additional details.") else displayName(item.detail))
            }
        }
    }
    decision?.let { action ->
        AttentionConfirmation(item, action, enabled, onDismiss = { decision = null }) {
            decision = null
            session.resolveAttention(item, action)
        }
    }
}

@Composable
private fun Location.description(spaces: List<Space>): String =
    displayName(spaces.spaceOf(this)?.name ?: storageId) + ": /" + displayName(path)

@Composable
private fun AttentionReason.explanation(): String = when (this) {
    AttentionReason.InterruptedWrite -> tr(
        "Запись прервалась при сбое или остановке. Сравните содержимое исходной папки и назначения; часть изменений могла сохраниться. Автоматического продолжения нет.",
        "A write was interrupted by a crash or stop. Compare source and destination contents; some changes may have persisted. There is no automatic continuation.")
    AttentionReason.ReplacementBackup -> tr(
        "Замена файла не завершилась однозначно. Прежняя версия могла остаться в скрытой резервной копии. Проверьте файл назначения, прежде чем решать судьбу копии.",
        "File replacement did not finish conclusively. The previous version may remain in a hidden backup. Inspect the destination file before deciding what to do with the backup.")
    AttentionReason.PartialTree -> tr(
        "Операция с папкой или перенос между пространствами остановились на середине. Изменена могла быть только часть объектов. Осмотрите обе папки; отметка «Разобрано» ничего не доделывает и не откатывает.",
        "A folder operation or transfer between spaces stopped part-way. Only some entries may have changed. Inspect both folders; marking reviewed neither completes nor rolls back the operation.")
    AttentionReason.PreparedPublication -> tr(
        "Публикация подготовленных файлов прервалась. Если доступно принятие результата, приложение заново проверит опубликованные данные. Остальные случаи нужно разобрать вручную по содержимому папок.",
        "Publishing prepared files was interrupted. When acceptance is available, the app rereads and verifies the published data. Other cases require manual inspection of folder contents.")
    AttentionReason.Cryptomator -> tr(
        "Переименование или изменение структуры Cryptomator прервалось. Не меняйте служебные зашифрованные имена вручную. Проверьте доступные данные через хранилище; автоматическое восстановление этого случая пока недоступно.",
        "A Cryptomator rename or restructuring was interrupted. Do not edit internal encrypted names manually. Inspect accessible data through the vault; automatic recovery for this case is not yet available.")
    AttentionReason.Other -> tr(
        "Приложение не смогло подтвердить завершение или откат операции. Осмотрите затронутые папки и подробности, затем выберите доступное решение.",
        "The app could not confirm completion or rollback. Inspect the affected folders and details before choosing an available action.")
}

@Composable
private fun AttentionConfirmation(item: AttentionItem, action: AttentionAction, enabled: Boolean,
    onDismiss: () -> Unit, onConfirm: () -> Unit) {
    var inspected by remember(item, action) { mutableStateOf(false) }
    val title = when (action) {
        AttentionAction.Restore -> tr("Вернуть прежнюю версию?", "Restore previous version?")
        AttentionAction.Discard -> tr("Удалить резервную копию?", "Delete the backup?")
        AttentionAction.Accept -> tr("Проверить и принять результат?", "Verify and accept result?")
        AttentionAction.Reviewed -> tr("Завершить разбор задания?", "Finish reviewing this job?")
    }
    AlertDialog(onDismissRequest = onDismiss, title = { Text(title) }, text = {
        Column(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(item.job.kind.label() + " · " + displayName(item.job.id))
            item.job.target?.let { Text(tr("Объект: /", "Entry: /") + displayName(it.path)) }
            Text(when (action) {
                AttentionAction.Restore -> tr(
                    "Резервная копия вернётся на прежнее имя только если оно свободно. Существующий файл не перезаписывается. Дождитесь успеха отдельного задания.",
                    "The backup is restored under the previous name only if that name is free. No existing file is overwritten. Wait for the separate job to succeed.")
                AttentionAction.Discard -> tr(
                    "Будет удалена скрытая прежняя версия файла. Убедитесь, что новый файл на месте и его содержимое вам подходит. Восстановление удалённой копии не гарантируется.",
                    "The hidden previous file version will be deleted. Make sure the new file is present and its contents are satisfactory. Recovery of the deleted backup is not guaranteed.")
                AttentionAction.Accept -> tr(
                    "Приложение перечитает опубликованные файлы и примет результат только при совпадении с подготовленными данными. Это проверка и принятие, а не повтор исходной операции.",
                    "The app rereads published files and accepts the result only if they match the prepared data. This verifies and accepts; it does not rerun the original operation.")
                AttentionAction.Reviewed -> tr(
                    "Задание исчезнет из списка требующих решения. Файлы не изменятся: приложение ничего не продолжит, не восстановит и не удалит. Оставшаяся резервная копия сама не очищается.",
                    "The job leaves the list of pending decisions. Files do not change: nothing is continued, restored or deleted. Any remaining backup is not automatically removed.")
            })
            item.backup?.let { Text(tr("Копия: /", "Backup: /") + displayName(it.path)) }
            if (action == AttentionAction.Discard || action == AttentionAction.Reviewed) {
                LabeledCheckbox(tr("Я проверил(а) содержимое и понимаю последствия", "I inspected the contents and understand the consequences"),
                    checked = inspected, enabled = enabled, onChange = { inspected = it })
            }
        }
    }, confirmButton = {
        TextButton(enabled = enabled && (action !in listOf(AttentionAction.Discard, AttentionAction.Reviewed) || inspected),
            onClick = onConfirm) { Text(title.removeSuffix("?"), color = if (action == AttentionAction.Discard)
                MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary) }
    }, dismissButton = { TextButton(onClick = onDismiss) { Text(tr("Отмена", "Cancel")) } })
}
