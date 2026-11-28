package io.github.stratayun.paranoic.ui.screens.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.resources.*
import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException

/**
 * Diagnostics after the owner's concept (27.11): a header, segmented tabs Overview / Checks /
 * Tasks; Overview sums up the system state, device storage, sync and the latest events.
 */
@Composable
fun DiagnosticsAndTasks(runningTasks: Int, startWithTasks: Boolean = false,
    diagnostics: @Composable () -> Unit, tasks: @Composable () -> Unit,
    overview: (@Composable (openChecks: () -> Unit) -> Unit)? = null) {
    var tab by remember { mutableIntStateOf(if (startWithTasks) 2 else if (overview != null) 0 else 1) }
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionHero(Res.drawable.ic_diagnostics, tr("Диагностика", "Diagnostics"),
            tr("Отчёт о системе, проверка данных и события приложения", "System report, data checks and app events"),
            Modifier.padding(top = 8.dp).sectionHeading(), iconSize = 64.dp)
        val labels = listOfNotNull(tr("Обзор", "Overview").takeIf { overview != null }, tr("Проверки", "Checks"),
            if (runningTasks > 0) tr("Задачи · $runningTasks", "Tasks · $runningTasks") else tr("Задачи", "Tasks"))
        val offset = if (overview != null) 0 else 1
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            labels.forEachIndexed { i, label ->
                SegmentedButton(selected = tab == i + offset, onClick = { tab = i + offset },
                    shape = SegmentedButtonDefaults.itemShape(i, labels.size)) { Text(label, maxLines = 1) }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (tab) {
                0 -> overview?.invoke { tab = 1 }
                1 -> diagnostics()
                else -> tasks()
            }
        }
    }
}

/** Overview tab: system state, device storage, sync and the latest Core events. */
@Composable
fun DiagnosticsOverview(session: AppSession, openChecks: () -> Unit, onSync: () -> Unit) {
    val core = session.core
    var events by remember { mutableStateOf<List<Diagnostic>>(emptyList()) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var pairs by remember { mutableStateOf<Int?>(null) }
    LaunchedEffect(core) {
        try {
            when (val recent = core.diagnostics()) {
                is CoreResult.Value -> events = recent.value.sortedByDescending { it.atMs }.take(4)
                is CoreResult.Failure -> fault = recent.fault
            }
            pairs = (core.syncPairs() as? CoreResult.Value)?.value?.size
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
    }
    val attention = session.jobs.count { it.phase == JobPhase.NeedsAttention }
    val running = session.jobs.count { it.phase == JobPhase.Queued || it.phase == JobPhase.Running }
    val healthy = fault == null && attention == 0 && core.connected
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        ConceptCard {
            ConceptRow(if (healthy) tr("Система в порядке", "System is fine") else tr("Требует внимания", "Needs attention"),
                when {
                    !core.connected -> tr("Ядро не подключено", "Core is not connected")
                    attention > 0 -> tr("Операций, требующих внимания: $attention", "Operations needing attention: $attention")
                    fault != null -> fault!!.label()
                    running > 0 -> tr("Выполняется операций: $running", "Running operations: $running")
                    else -> tr("Критичных проблем не найдено", "No critical problems found")
                }, if (healthy) Res.drawable.ic_check else Res.drawable.ic_diagnostics)
        }
        val device = session.files.spaces.userPlaces().firstOrNull { it.detachable && !it.cloud }
        val usage = device?.let { LocalVolumeUsage.current(it.name) }?.takeIf { it.totalBytes > 0 }
        if (device != null && usage != null) StorageUsageCard(tr("Память устройства", "Device storage"), usage, Res.drawable.ic_internal_storage)
        ConceptCard {
            ConceptRow(tr("Синхронизация", "Sync"), when (pairs) {
                null -> tr("Состояние неизвестно", "State unknown")
                0 -> tr("Пар пока нет", "No pairs yet")
                else -> tr("Пар: $pairs", "Pairs: $pairs")
            }, Res.drawable.ic_sync, onClick = onSync)
        }
        ConceptCard {
            ConceptCardTitle(tr("Проверки", "Checks"))
            ConceptRow(tr("Атомарная запись", "Atomic writes"), tr("Целостность и безопасность файлов", "File integrity and safety"),
                Res.drawable.ic_shield, onClick = openChecks)
            ConceptRow(tr("Целостность хранилищ", "Vault integrity"), tr("Крипто-папки и Сейф проверяются при каждом открытии", "Crypto folders and the Safe are verified on every opening"),
                Res.drawable.ic_crypto_folder)
            Button(onClick = openChecks, modifier = Modifier.fillMaxWidth()) {
                AppIcon(AppSymbol.Analytics); Spacer(Modifier.width(8.dp)); Text(tr("Запустить диагностику", "Run diagnostics"))
            }
        }
        ConceptCard {
            ConceptCardTitle(tr("Последние события", "Recent events"), tr("Показать все", "Show all"), openChecks)
            if (events.isEmpty()) Text(tr("Событий нет.", "No events."), Modifier.padding(8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            events.forEach { entry ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(fileModifiedLabel(entry.atMs.coerceAtLeast(0).toULong()).orEmpty(), style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(96.dp))
                    Text(entry.message, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, maxLines = 2)
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}
