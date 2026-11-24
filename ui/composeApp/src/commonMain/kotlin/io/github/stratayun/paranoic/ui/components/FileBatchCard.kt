package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.ui.state.*

@Composable
fun FileBatchCard(session: AppSession) {
    val report = session.fileBatch ?: return
    val jobs = session.jobs
    val summary = report.summary(jobs)
    var details by remember(report.entries.map { it.requestId }) { mutableStateOf(false) }
    var closing by remember { mutableStateOf(false) }
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Выбранные объекты", "Selected entries"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
            Text(tr("Итог известен: ${summary.settled} из ${summary.total}", "Outcome known: ${summary.settled} of ${summary.total}"), Modifier.announceChanges())
            LinearProgressIndicator(progress = { summary.settled.toFloat() / summary.total }, modifier = Modifier.fillMaxWidth())
            Text(tr("Готово: ${summary.done} · Пропущено: ${summary.skipped} · Ошибок: ${summary.failed}",
                "Done: ${summary.done} · Skipped: ${summary.skipped} · Failed: ${summary.failed}"))
            Text(tr("Отменено: ${summary.cancelled} · Требуют внимания: ${summary.attention} · Не отправлено: ${summary.notSent} · Неизвестно: ${summary.unknown}",
                "Cancelled: ${summary.cancelled} · Needs attention: ${summary.attention} · Not submitted: ${summary.notSent} · Unknown: ${summary.unknown}"))
            if (report.sending) {
                Text(if (report.stopRequested) tr("Ждём ответ на текущий запрос. Остаток отправлен не будет.", "Waiting for the current reply. Remaining entries will not be submitted.")
                    else tr("Отправляем задания ядру…", "Submitting jobs to Core…"))
                TextButton(enabled = !report.stopRequested, onClick = session::stopFileBatch) { Text(tr("Остановить отправку остатка", "Stop remaining submissions")) }
            }
            Text(tr("Принятые задания продолжаются независимо. Сводка доступна до перезапуска приложения; сами задания сохранены в журнале.",
                "Accepted jobs continue independently. This summary lasts until app restart; the jobs are saved in the journal."))
            TextButton(onClick = { details = !details }) { Text(if (details) tr("Свернуть список", "Collapse list") else tr("Результат по каждому объекту", "Per-entry results")) }
            if (details) LazyColumn(Modifier.fillMaxWidth().heightIn(max = 220.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                itemsIndexed(report.entries, key = { _, entry -> entry.requestId }) { index, entry ->
                    val job = jobs.firstOrNull { it.id == entry.jobId }
                    Column {
                        Text(displayName(entry.source?.name ?: job?.source?.path?.substringAfterLast('/') ?: tr("Объект ${index + 1}", "Entry ${index + 1}")))
                        Text(entry.status(jobs).batchLabel(), style = MaterialTheme.typography.bodySmall)
                        entry.fault?.let { FaultNotice(it) }
                        job?.error?.let { JobFaultNotice(it) }
                        Text(tr("Запрос: ", "Request: ") + entry.requestId, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
            TextButton(enabled = session.core.connected && !session.jobsLoading, onClick = session::refreshJobs) { Text(tr("Проверить журнал", "Check journal")) }
            if (!report.sending) TextButton(onClick = { if (summary.unknown > 0) closing = true else session.dismissFileBatch() }) {
                Text(tr("Закрыть сводку", "Close summary"))
            }
        }
    }
    if (closing) AlertDialog(onDismissRequest = { closing = false }, title = { Text(tr("Есть неизвестный результат", "Some outcomes are unknown")) },
        text = { Text(tr("Сначала проверьте журнал и папки. Закрытие уберёт только эту сводку: задания не отменятся и не повторятся. Повторный запуск с новым запросом может продублировать действие.",
            "Inspect the journal and folders first. Closing removes only this summary: jobs are neither cancelled nor repeated. Starting a new request may duplicate an action.")) },
        confirmButton = { TextButton(onClick = { closing = false; session.dismissFileBatch() }) { Text(tr("Проверил(а), закрыть", "Inspected, close")) } },
        dismissButton = { TextButton(onClick = { closing = false }) { Text(tr("Вернуться", "Go back")) } })
}

@Composable
private fun FileBatchStatus.batchLabel() = when (this) {
    FileBatchStatus.Sending -> tr("Ещё не принят ядром", "Not yet accepted by Core")
    FileBatchStatus.Queued -> tr("В очереди", "Queued")
    FileBatchStatus.Running -> tr("Выполняется", "Running")
    FileBatchStatus.Done -> tr("Выполнено", "Done")
    FileBatchStatus.Skipped -> tr("Пропущено без записи", "Skipped without writing")
    FileBatchStatus.Failed -> tr("Ошибка", "Failed")
    FileBatchStatus.Cancelled -> tr("Отменено", "Cancelled")
    FileBatchStatus.NeedsAttention -> tr("Требует решения в заданиях", "Needs a decision in Tasks")
    FileBatchStatus.Unknown -> tr("Неизвестно — проверьте журнал", "Unknown — check the journal")
    FileBatchStatus.NotSent -> tr("Не отправлено", "Not submitted")
}
