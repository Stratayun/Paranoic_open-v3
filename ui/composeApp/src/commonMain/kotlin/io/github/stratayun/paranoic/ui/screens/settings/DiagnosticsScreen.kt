package io.github.stratayun.paranoic.ui.screens.settings

import io.github.stratayun.paranoic.ui.components.*
import io.github.stratayun.paranoic.resources.*
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
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item { ConceptCard { RenameProbeSection(session) } }
        item {
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it) }
            if (crashUnreadable) Text(tr("Сохранённый сбой прочитать не удалось; он не заменён пустой записью.", "The saved crash could not be read; it was not replaced with an empty record."), color = MaterialTheme.colorScheme.error)
            if (crash != null) Text(tr("Есть запись предыдущего сбоя Kotlin/JVM.", "A previous Kotlin/JVM crash was recorded."), Modifier.announceChanges())
        }
        bundle?.let { report ->
            item {
                ConceptCard {
                    ConceptCardTitle(tr("Отчёт", "Report"))
                    Text(tr("Технические сведения и журнал ядра. Пароли, имена файлов, пути и содержимое не входят.",
                        "Technical facts and the core journal. No passwords, file names, paths or content."),
                        Modifier.padding(horizontal = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    val folder = session.files.folder
                    ActionTiles(
                        Triple(tr("Скопировать", "Copy"), AppSymbol.Copy, true to {
                            try { clipboard.setText(AnnotatedString(report.text)); copied = true }
                            catch (_: Exception) { fault = CoreFault.Unavailable }
                        }),
                        Triple(tr("Сохранить", "Save"), AppSymbol.Save, (folder != null && session.files.loaded && session.files.writable && !session.writing &&
                            session.directWrite?.phase != DirectWritePhase.Unknown) to { if (folder != null) session.saveDiagnosticReport(folder, report) }),
                        Triple(tr("Папка", "Folder"), AppSymbol.Folder, true to onChooseFolder),
                        Triple(if (fullReport) tr("Свернуть", "Collapse") else tr("Показать", "Show"), AppSymbol.Document, true to { fullReport = !fullReport }),
                    )
                    if (copied) Text(tr("Отчёт скопирован.", "Report copied."), Modifier.padding(horizontal = 8.dp))
                    if (folder != null) Text(tr("Сохранение в: /", "Saves to: /") + displayName(folder.path), Modifier.padding(horizontal = 8.dp),
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    if (fullReport) Text(report.text, Modifier.padding(8.dp), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        item {
            ConceptCardTitle(tr("Последние события ядра", "Recent Core events"), tr("Обновить", "Refresh"), { if (!loading && !clearing) revision++ })
            Text(tr("Это общий журнал; события могут относиться к разным операциям.", "This journal is shared; events can belong to different operations."))
            Text(tr("Журнал очищается после Clear и каждого входа в Сейф. Ваши сохранённые отчёты остаются.",
                "The journal clears after Clear and each Safe sign-in. Reports you saved stay."), style = MaterialTheme.typography.bodySmall)
            TextButton(enabled = !loading && !clearing && (diagnostics.isNotEmpty() || crash != null || crashUnreadable), onClick = { confirmClear = true }) { Text(tr("Очистить журнал и запись сбоя", "Clear journal and crash record")) }
            if (diagnostics.isEmpty() && !loading) Text(tr("Событий нет.", "No events."))
        }
        items(diagnostics.sortedByDescending { it.atMs }, key = null) { entry ->
            ConceptRow(entry.message, listOfNotNull(entry.code, fileModifiedLabel(entry.atMs.coerceAtLeast(0).toULong())).joinToString(" · "))
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

/** Diagnostics: atomic "rename without replacing" check per device place (audit finding 1). */
@Composable
private fun RenameProbeSection(session: AppSession) {
    val scope = rememberCoroutineScope()
    val places = session.files.spaces.userPlaces().filter { !it.cloud }
    val results = remember { mutableStateMapOf<String, RenameProbe?>() }
    var running by remember { mutableStateOf<String?>(null) }
    val scheme = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(tr("Проверка атомарной записи", "Atomic write check"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleMedium)
        Text(tr("Проверяет, может ли память переименовать файл без замены существующего. Создаёт и удаляет два пробных файла.",
            "Checks whether storage can rename without replacing an existing file. Creates and removes two test files."),
            style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        if (places.isEmpty()) Text(tr("Нет подключённой памяти устройства.", "No device storage connected."))
        places.forEach { space ->
            Surface(shape = MaterialTheme.shapes.large, color = scheme.surfaceContainerHigh) {
                Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Text(displayName(space.name), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                        FilledTonalButton(enabled = running == null && session.core.connected, onClick = {
                            running = space.id
                            scope.launch {
                                results[space.id] = (session.core.renameProbe(space) as? CoreResult.Value)?.value ?: RenameProbe.Failed
                                running = null
                            }
                        }) { Text(if (running == space.id) tr("Проверяем…", "Checking…") else tr("Проверить", "Check")) }
                    }
                    results[space.id]?.let { result ->
                        val (text, color) = when (result) {
                            RenameProbe.Supported -> tr("✓ Поддерживается: запись атомарна и никогда не заменяет чужой файл.",
                                "✓ Supported: writes are atomic and never replace another file.") to scheme.primary
                            RenameProbe.Unsupported -> tr("⚠ Не поддерживается: в редкой гонке с другим приложением возможна замена файла.",
                                "⚠ Not supported: a rare race with another app may replace a file.") to scheme.tertiary
                            RenameProbe.Ignored -> tr("✕ Опасно: флаг игнорируется и существующий файл ЗАМЕНЯЕТСЯ. Сообщите разработчику.",
                                "✕ Dangerous: the flag is ignored and an existing file IS REPLACED. Report this.") to scheme.error
                            RenameProbe.Failed -> tr("Проверка не выполнилась (нет записи или ошибка ввода-вывода).", "The check could not run (no write access or I/O error).") to scheme.error
                            RenameProbe.NotLocal -> tr("Не локальная папка — проверка неприменима.", "Not a local folder — not applicable.") to scheme.onSurfaceVariant
                        }
                        Text(text, style = MaterialTheme.typography.bodyMedium, color = color)
                    }
                }
            }
        }
    }
}
