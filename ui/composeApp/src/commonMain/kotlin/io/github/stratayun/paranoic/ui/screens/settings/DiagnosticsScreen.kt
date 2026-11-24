package io.github.stratayun.paranoic.ui.screens.settings

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@OptIn(kotlin.time.ExperimentalTime::class)
@Composable
fun DiagnosticsScreen(session: AppSession, environment: DiagnosticEnvironment, onChooseFolder: () -> Unit) {
    val core = session.core
    val scope = rememberCoroutineScope()
    @Suppress("DEPRECATION") val clipboard = LocalClipboardManager.current
    var revision by remember { mutableIntStateOf(0) }
    var loading by remember { mutableStateOf(false) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var diagnostics by remember { mutableStateOf<List<Diagnostic>>(emptyList()) }
    var bundle by remember { mutableStateOf<DiagnosticBundle?>(null) }
    var crash by remember { mutableStateOf<String?>(null) }
    var crashUnreadable by remember { mutableStateOf(false) }
    var fullReport by remember { mutableStateOf(false) }
    var confirmClear by remember { mutableStateOf(false) }
    var clearing by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    LaunchedEffect(core, environment, revision) {
        loading = true; fault = null; bundle = null; copied = false; crashUnreadable = false
        try {
            val recent = core.diagnostics()
            diagnostics = if (recent is CoreResult.Value) recent.value.takeLast(100) else emptyList()
            if (recent is CoreResult.Failure) fault = recent.fault
            val report = core.diagnosticsReportText()
            if (report is CoreResult.Failure) fault = report.fault
            crash = try { environment.previousCrash() }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { crashUnreadable = true; null }
            val facts = environment.facts()
            val coreText = when (report) { is CoreResult.Value -> report.value; is CoreResult.Failure -> null }
            bundle = diagnosticBundle(kotlin.time.Clock.System.now().toEpochMilliseconds(), facts,
                coreText, diagnostics, crash)
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { fault = CoreFault.Unavailable }
        finally { loading = false }
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Text(tr("Диагностика", "Diagnostics"), Modifier.sectionHeading(), style = MaterialTheme.typography.headlineSmall)
            Text(tr("Отчёт содержит технические сведения, общий журнал ядра и сохранённый сбой Kotlin/JVM. Пароли, содержимое документов, имена файлов и пути в него не включаются.",
                "The report contains technical information, the shared Core journal and a saved Kotlin/JVM crash. It excludes passwords, document contents, file names and paths."))
            TextButton(enabled = !loading && !clearing, onClick = { revision++ }) { Text(tr("Обновить", "Refresh")) }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it) }
            if (crashUnreadable) Text(tr("Сохранённый сбой прочитать не удалось; он не заменён пустой записью.", "The saved crash could not be read; it was not replaced with an empty record."), color = MaterialTheme.colorScheme.error)
            if (crash != null) Text(tr("Есть запись предыдущего сбоя Kotlin/JVM.", "A previous Kotlin/JVM crash was recorded."), Modifier.announceChanges())
        }
        bundle?.let { report ->
            item {
                TextButton(onClick = {
                    try { clipboard.setText(AnnotatedString(report.text)); copied = true }
                    catch (_: Exception) { fault = CoreFault.Unavailable }
                }) { Text(tr("Скопировать отчёт", "Copy report")) }
                if (copied) Text(tr("Отчёт скопирован.", "Report copied."))
                val folder = session.files.folder
                TextButton(enabled = folder != null && session.files.loaded && session.files.writable && !session.writing &&
                    session.directWrite?.phase != DirectWritePhase.Unknown, onClick = { if (folder != null) session.saveDiagnosticReport(folder, report) }) {
                    Text(tr("Сохранить отчёт в текущей папке файлов", "Save report to the current Files folder"))
                }
                TextButton(onClick = onChooseFolder) { Text(tr("Выбрать папку в «Файлах»", "Choose a folder in Files")) }
                if (folder != null) Text(tr("Папка назначения: /", "Destination folder: /") + displayName(folder.path))
                TextButton(onClick = { fullReport = !fullReport }) { Text(if (fullReport) tr("Свернуть отчёт", "Collapse report") else tr("Показать отчёт", "Show report")) }
            }
            if (fullReport) item { Text(report.text) }
        }
        item {
            Text(tr("Последние события ядра", "Recent Core events"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
            Text(tr("Это общий журнал; события могут относиться к разным операциям.", "This journal is shared; events can belong to different operations."))
            Text(tr("Журнал и запись сбоя очищаются после Clear и каждого успешного входа в Сейф. Сохранённые вами отчёты при этом не удаляются.",
                "The journal and crash record are cleared after Clear and every successful Safe unlock. Reports you saved are not removed."), style = MaterialTheme.typography.bodySmall)
            TextButton(enabled = !loading && !clearing && (diagnostics.isNotEmpty() || crash != null || crashUnreadable), onClick = { confirmClear = true }) { Text(tr("Очистить журнал и запись сбоя", "Clear journal and crash record")) }
            if (diagnostics.isEmpty() && !loading) Text(tr("Событий нет.", "No events."))
        }
        items(diagnostics.sortedByDescending { it.atMs }, key = null) { entry ->
            OutlinedCard(Modifier.fillMaxWidth()) { Column(Modifier.padding(12.dp)) {
                Text(entry.code); Text(entry.message)
                fileModifiedLabel(entry.atMs.coerceAtLeast(0).toULong())?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            } }
        }
    }
    if (confirmClear) AlertDialog(onDismissRequest = { confirmClear = false }, title = { Text(tr("Очистить диагностику?", "Clear diagnostics?")) },
        text = { Text(tr("Удалятся события ядра и сохранённая запись сбоя. Файлы, заметки и задания не изменятся.", "Core events and the saved crash record will be removed. Files, notes and jobs are unchanged.")) },
        confirmButton = { TextButton(onClick = {
            confirmClear = false; clearing = true
            scope.launch {
                try {
                    when (val result = core.clearDiagnostics()) {
                        is CoreResult.Value -> { environment.clearCrash(); revision++ }
                        is CoreResult.Failure -> fault = result.fault
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { fault = CoreFault.Unavailable }
                finally { clearing = false }
            }
        }) { Text(tr("Очистить", "Clear")) } }, dismissButton = { TextButton(onClick = { confirmClear = false }) { Text(tr("Отмена", "Cancel")) } })
}
