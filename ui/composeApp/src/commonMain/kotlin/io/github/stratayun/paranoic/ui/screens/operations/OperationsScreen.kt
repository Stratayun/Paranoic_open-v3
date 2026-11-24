package io.github.stratayun.paranoic.ui.screens.operations

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.JobKind
import io.github.stratayun.paranoic.bridge.JobPhase
import io.github.stratayun.paranoic.bridge.JobOutcome
import io.github.stratayun.paranoic.bridge.Location
import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.ui.components.FileBatchCard
import io.github.stratayun.paranoic.ui.components.JobFaultNotice
import io.github.stratayun.paranoic.ui.theme.LocalThemeTokens

/** Only journal snapshots from the new Core. Cancellation is a request, not a result. */
@Composable
@OptIn(ExperimentalLayoutApi::class)
fun OperationsScreen(session: AppSession, onOpenFolder: (Location) -> Unit = { session.openAttentionFolder(it) }) {
    var attentionOnly by remember { mutableStateOf(true) }
    LaunchedEffect(session) { session.refreshJobs(); session.refreshAttention() }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(LocalThemeTokens.current.pagePadding),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            FileBatchCard(session)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(selected = attentionOnly, onClick = { attentionOnly = true },
                    label = { Text(tr("Требуют внимания", "Needs attention")) })
                FilterChip(selected = !attentionOnly, onClick = { attentionOnly = false },
                    label = { Text(tr("Все задания", "All jobs")) })
            }
            TextButton(onClick = { session.refreshJobs(); session.refreshAttention() },
                enabled = session.core.connected && !session.jobsLoading && !session.attention.loading) {
                Text(tr("Обновить", "Refresh"))
            }
        }
        if (attentionOnly) {
            if (session.attention.loading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
            session.attention.fault?.let { fault -> item { FaultNotice(fault) } }
            if (session.attention.loaded && !session.attention.loading && session.attention.fault == null && session.attention.items.isEmpty())
                item {
                    Text(tr("Нет заданий, требующих решения.", "No jobs need a decision."))
                    TextButton(onClick = { attentionOnly = false }) { Text(tr("Показать все задания", "Show all jobs")) }
                }
            items(session.attention.items, key = { "attention:" + it.job.id }) { item ->
                AttentionCard(session, item, onOpenFolder)
            }
            return@LazyColumn
        }
        if (session.jobsLoading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
        session.jobsFault?.let { fault -> item { FaultNotice(fault) } }
        if (session.jobsLoaded && session.jobs.isEmpty()) item { Text(tr("Заданий пока нет. После копирования, переноса или удаления здесь появятся ход работы и результат.",
            "No jobs yet. Copying, moving and deletion add their progress and results here.")) }
        items(session.jobs, key = { it.id }) { job ->
            Surface(shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
                Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(job.kind.label() + " · " + job.phase.label(), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
                    job.source?.let { Text(tr("Откуда: ", "From: ") + displayName("/" + it.path)) }
                    job.target?.let { Text((if (job.kind == JobKind.Copy) tr("Запрошенное назначение: ", "Requested destination: ")
                        else tr("Куда: ", "To: ")) + displayName("/" + it.path)) }
                    if (job.phase == JobPhase.Succeeded) {
                        Text(when (job.outcome) {
                            JobOutcome.Done -> tr("Сделано", "Done")
                            JobOutcome.Skipped -> tr("Пропущено: имя занято, ничего не записано.", "Skipped: the name exists; nothing was written.")
                            JobOutcome.Unknown -> tr("Подробный исход не передан.", "Detailed outcome is unavailable.")
                        })
                        if (job.outcome == JobOutcome.Done) job.resultTarget?.let {
                            Text(tr("Фактическое назначение: /", "Actual destination: /") + displayName(it.path))
                        }
                    }
                    job.error?.let { JobFaultNotice(it) }
                    Text(tr("Обработано байт: ${job.bytesDone}", "Bytes processed: ${job.bytesDone}"))
                    job.bytesTotal?.let { total ->
                        if (total > 0uL) LinearProgressIndicator(
                            progress = { (job.bytesDone.toDouble() / total.toDouble()).toFloat().coerceIn(0f, 1f) },
                            modifier = Modifier.fillMaxWidth())
                    }
                    if (job.canCancel) TextButton(onClick = { session.cancel(job) }, enabled = !session.writing) {
                        Text(tr("Запросить отмену", "Request cancellation"))
                    }
                }
            }
        }
    }
}
