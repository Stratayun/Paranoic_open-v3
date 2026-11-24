package io.github.stratayun.paranoic.ui.screens.sync

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

private enum class SyncDecision { Run, Resume, Remove, Local, Remote, Forget }
private data class PendingDecision(val pair: SyncPair, val kind: SyncDecision, val path: String = "")

/** Explicit plans and decisions only. Blocking IO is dispatched by CoreGateway. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SyncScreen(core: CoreGateway, onFiles: () -> Unit, onJobs: () -> Unit) {
    val scope = rememberCoroutineScope()
    var pairs by remember(core) { mutableStateOf<List<SyncPair>>(emptyList()) }
    var spaces by remember(core) { mutableStateOf<List<Space>>(emptyList()) }
    var selectedId by remember(core) { mutableStateOf<String?>(null) }
    var plan by remember(core) { mutableStateOf<SyncPlanSummary?>(null) }
    var outcome by remember(core) { mutableStateOf<SyncRunResult?>(null) }
    var fault by remember(core) { mutableStateOf<CoreFault?>(null) }
    var busy by remember(core) { mutableStateOf(false) }
    var uncertain by remember(core) { mutableStateOf(false) }
    var loaded by remember(core) { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    var decision by remember { mutableStateOf<PendingDecision?>(null) }

    fun fail(value: CoreFault, write: Boolean = false) {
        fault = value
        if (write && value in setOf(CoreFault.OutcomeUnknown, CoreFault.Unavailable,
                CoreFault.Disconnected, CoreFault.Cancelled)) uncertain = true
    }
    suspend fun refresh() {
        when (val result = syncSafe { core.syncPairs() }) {
            is CoreResult.Failure -> fail(result.fault)
            is CoreResult.Value -> {
                pairs = result.value
                if (pairs.none { it.id == selectedId }) selectedId = pairs.firstOrNull()?.id
                loaded = true
                uncertain = false
            }
        }
        when (val result = syncSafe { core.spaces() }) {
            is CoreResult.Value -> spaces = result.value
            is CoreResult.Failure -> if (fault == null) fail(result.fault)
        }
    }
    fun act(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        fault = null
        scope.launch { try { block() } finally { busy = false } }
    }
    LaunchedEffect(core) { act { refresh() } }
    val pair = pairs.firstOrNull { it.id == selectedId }
    val missingKey = fault == CoreFault.DeviceKey
    val canChange = !busy && !uncertain && !missingKey && core.connected
    val reports = outcome?.reports ?: plan?.reports.orEmpty()
    // Plan-only conflicts are recorded by Core after conflict-copy jobs finish.
    val conflicts = pair?.conflicts.orEmpty().distinct()

    BoxWithConstraints(Modifier.fillMaxSize()) {
    val controlsHeight = maxHeight * 0.45f
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(Modifier.fillMaxWidth().heightIn(max = controlsHeight).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(tr("Синхронизация и бэкап", "Sync and backup"), Modifier.sectionHeading(), style = MaterialTheme.typography.titleLarge)
        if (busy) {
            LinearProgressIndicator(Modifier.fillMaxWidth())
            Text(tr("Операция выполняется. Задания копирования видны в «Операциях».",
                "Processing. Copy jobs appear in Operations."))
        }
        fault?.let {
            FaultNotice(it, message = when (it) {
                CoreFault.DeviceKey -> tr("Синхронизация недоступна без ключа устройства (Keystore/DPAPI). На desktop Linux/macOS этот ключ пока не поддерживается.",
                    "Sync requires a device key (Keystore/DPAPI). This key is not yet supported on desktop Linux/macOS.")
                CoreFault.InvalidName -> tr("Пара, папка или сохранённый план недоступны. Для новой пары нужны локальная и подключённая облачная папки; перед запуском создайте план.",
                    "The pair, folder or saved plan is unavailable. New pairs require a local and a connected cloud folder; create a plan before running.")
                CoreFault.Busy -> tr("Старый план уже отправлял задания. Сначала завершите его через «Продолжить сохранённый план»; затем можно строить новый план и принимать решения.",
                    "The previous plan already submitted jobs. Finish it using Resume saved plan before planning again or making decisions.")
                else -> it.label()
            })
        }
        if (uncertain) Text(tr("Ответ неизвестен. Проверьте задания и обновите список перед новой операцией. Автоматического повтора нет.",
            "The reply is unknown. Inspect jobs and refresh the list before another operation. There is no automatic retry."),
            color = MaterialTheme.colorScheme.error)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(enabled = !busy, onClick = { act { plan = null; outcome = null; refresh() } }) { Text(tr("Обновить", "Refresh")) }
            TextButton(enabled = canChange && loaded, onClick = { adding = true }) { Text(tr("Добавить пару", "Add pair")) }
            TextButton(onClick = onJobs) { Text(tr("К заданиям", "View jobs")) }
            TextButton(enabled = !busy, onClick = onFiles) { Text(tr("К файлам", "View files")) }
        }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (loaded && pairs.isEmpty()) item { Text(tr("Пар пока нет. Подключите папку и облако в разделе файлов, затем добавьте пару.",
                "No pairs yet. Connect a folder and a cloud in Files, then add a pair.")) }
            items(pairs, key = { it.id }) { item ->
                OutlinedCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp)) {
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().selectable(selectedId == item.id,
                            enabled = !busy, role = Role.RadioButton, onClick = { selectedId = item.id; plan = null; outcome = null }),
                            verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = selectedId == item.id, onClick = null)
                            Text(if (item.twoWay) tr("Двусторонняя", "Two-way") else tr("Бэкап: локально → облако", "Backup: local → cloud"))
                        }
                        Text(tr("Локально: ", "Local: ") + locationLabel(item.local, spaces))
                        Text(tr("Облако: ", "Cloud: ") + locationLabel(item.remote, spaces))
                        if (item.trashRemoteDeletions) Text(tr("Удалённое локально — в корзину облака", "Local deletions go to cloud trash"))
                        TextButton(enabled = canChange, onClick = { decision = PendingDecision(item, SyncDecision.Remove) }) {
                            Text(tr("Убрать пару", "Remove pair"))
                        }
                    }
                }
            }
            if (pair != null) {
                item {
                    Text(tr("Выбранная пара", "Selected pair"), style = MaterialTheme.typography.titleMedium)
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(enabled = canChange, onClick = { act {
                            outcome = null
                            plan = null
                            when (val result = syncSafe { core.syncPlan(pair.id) }) {
                                is CoreResult.Value -> plan = result.value
                                is CoreResult.Failure -> fail(result.fault, write = true)
                            }
                        } }) { Text(tr("План", "Plan")) }
                        Button(enabled = canChange && plan != null,
                            onClick = { decision = PendingDecision(pair, SyncDecision.Run) }) { Text(tr("Выполнить", "Run")) }
                        TextButton(enabled = canChange, onClick = { decision = PendingDecision(pair, SyncDecision.Resume) }) {
                            Text(tr("Продолжить сохранённый план…", "Resume saved plan…"))
                        }
                    }
                    Text(tr("После прерывания используйте сохранённый план: уже принятые задания не повторяются. План и запуск могут занять время.",
                        "After an interruption, resume the saved plan: accepted jobs are not repeated. Planning and running may take time."),
                        style = MaterialTheme.typography.bodySmall)
                    Text(tr("«План» сравнивает папки и показывает изменения, не изменяя файлы.",
                        "Plan compares folders and previews changes without modifying files."), style = MaterialTheme.typography.bodySmall)
                }
                plan?.let { preview ->
                    item { Text(tr("Действия (${preview.actions.size})", "Actions (${preview.actions.size})"), style = MaterialTheme.typography.titleMedium) }
                    items(preview.actions) { action ->
                        Column { Text(actionLabel(action.kind)); Text(displayName(action.path), style = MaterialTheme.typography.bodySmall) }
                    }
                }
                outcome?.let { result -> item {
                    Text(tr("Завершено заданий: ${result.done}; с ошибкой: ${result.failed}.",
                        "Jobs completed: ${result.done}; failed: ${result.failed}."))
                } }
                if (conflicts.isNotEmpty()) item { Text(tr("Конфликты", "Conflicts"), style = MaterialTheme.typography.titleMedium) }
                items(conflicts) { path ->
                    OutlinedCard(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(12.dp)) {
                            Text(displayName(path))
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                TextButton(enabled = canChange, onClick = { decision = PendingDecision(pair, SyncDecision.Local, path) }) {
                                    Text(tr("Оставить моё", "Keep local"))
                                }
                                TextButton(enabled = canChange, onClick = { decision = PendingDecision(pair, SyncDecision.Remote, path) }) {
                                    Text(tr("Оставить облачное", "Keep cloud"))
                                }
                            }
                        }
                    }
                }
                if (reports.any { it.kind != 1 || it.path !in conflicts }) item { Text(tr("Предупреждения", "Warnings"), style = MaterialTheme.typography.titleMedium) }
                items(reports.filter { it.kind != 1 || it.path !in conflicts }) { report ->
                    Column {
                        Text(reportLabel(report.kind))
                        Text(displayName(report.path), style = MaterialTheme.typography.bodySmall)
                        if (report.kind == 1) Text(tr("Сначала выполните план: облачная версия сохранится локальной конфликтной копией. После завершения появится выбор версии.",
                            "Run the plan first: the cloud version is saved as a local conflict copy. Version choices appear when the run finishes."),
                            style = MaterialTheme.typography.bodySmall)
                        if (report.kind == 2 || report.kind == 3) TextButton(enabled = canChange,
                            onClick = { decision = PendingDecision(pair, SyncDecision.Forget, report.path) }) { Text(tr("Принять…", "Accept…")) }
                    }
                }
            }
        }
    }
    }
    if (adding) AddSyncPair(core, spaces, onClose = { adding = false }, onAdd = { local, remote, twoWay, trash ->
        adding = false
        act {
            when (val result = syncSafe { core.syncPairAdd(local, remote, twoWay, trash) }) {
                is CoreResult.Value -> { selectedId = result.value.id; plan = null; outcome = null; refresh() }
                is CoreResult.Failure -> fail(result.fault, write = true)
            }
        }
    })
    decision?.let { pending ->
        AlertDialog(onDismissRequest = { decision = null },
            title = { Text(decisionTitle(pending.kind)) },
            text = { Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Локально: ", "Local: ") + locationLabel(pending.pair.local, spaces))
                Text(tr("Облако: ", "Cloud: ") + locationLabel(pending.pair.remote, spaces))
                if (pending.path.isNotEmpty()) Text(displayName(pending.path))
                Text(decisionDescription(pending.kind))
                if (pending.kind == SyncDecision.Run) Text(tr("Действий в плане: ${plan?.actions?.size ?: 0}. Замены и переносы в корзину будут выполнены согласно показанному плану.",
                    "Planned actions: ${plan?.actions?.size ?: 0}. Replacements and trash moves follow the displayed plan."))
            } },
            confirmButton = { TextButton(enabled = canChange, onClick = {
                decision = null
                act {
                    when (pending.kind) {
                        SyncDecision.Run, SyncDecision.Resume -> {
                            when (val result = syncSafe { core.syncRun(pending.pair.id) }) {
                                is CoreResult.Value -> { plan = null; outcome = result.value; refresh() }
                                is CoreResult.Failure -> fail(result.fault, write = true)
                            }
                        }
                        else -> {
                            val result = syncSafe { when (pending.kind) {
                                SyncDecision.Remove -> core.syncPairRemove(pending.pair.id)
                                SyncDecision.Local, SyncDecision.Remote -> core.syncResolve(pending.pair.id, pending.path, pending.kind == SyncDecision.Local)
                                SyncDecision.Forget -> core.syncForget(pending.pair.id, pending.path)
                                else -> CoreResult.Failure(CoreFault.Unsupported)
                            } }
                            when (result) {
                                is CoreResult.Value -> { plan = null; outcome = null; refresh() }
                                is CoreResult.Failure -> fail(result.fault, write = true)
                            }
                        }
                    }
                }
            }) { Text(tr("Подтвердить", "Confirm")) } },
            dismissButton = { TextButton(onClick = { decision = null }) { Text(tr("Отмена", "Cancel")) } })
    }
}

private suspend fun <T> syncSafe(block: suspend () -> CoreResult<T>): CoreResult<T> = try { block() }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { CoreResult.Failure(CoreFault.Unavailable) }

private fun locationLabel(location: Location, spaces: List<Space>): String =
    displayName(spaces.firstOrNull { it.id == location.storageId }?.name ?: location.storageId.take(8)) + " /" + displayName(location.path)

@Composable
private fun actionLabel(kind: Int) = when (kind) {
    1 -> tr("Создать локальную папку", "Create local folder")
    2 -> tr("Создать облачную папку", "Create cloud folder")
    3 -> tr("Загрузить в облако", "Upload")
    4 -> tr("Загрузить с заменой облачной версии", "Upload replacing cloud version")
    5 -> tr("Скачать", "Download")
    6 -> tr("Скачать с заменой локальной версии", "Download replacing local version")
    7 -> tr("Сохранить облачную версию локальной конфликтной копией", "Save cloud version as a local conflict copy")
    8 -> tr("Переместить в корзину облака", "Move to cloud trash")
    9 -> tr("Принять совпадающее содержимое без копирования", "Adopt equal content without copying")
    else -> tr("Неизвестное действие ($kind)", "Unknown action ($kind)")
}
@Composable
private fun reportLabel(kind: Int) = when (kind) {
    1 -> tr("Конфликт версий", "Version conflict")
    2 -> tr("Удалено локально", "Deleted locally")
    3 -> tr("Удалено в облаке", "Deleted in cloud")
    4 -> tr("Есть только в облаке", "Only in cloud")
    5 -> tr("Изменено в облаке", "Changed in cloud")
    6 -> tr("Типы различаются: файл/папка", "File/folder type mismatch")
    7 -> tr("Неподдерживаемый объект", "Unsupported entry")
    8 -> tr("Изменилось до постановки задания", "Changed before job submission")
    else -> tr("Неизвестное предупреждение ($kind)", "Unknown warning ($kind)")
}
@Composable
private fun decisionTitle(kind: SyncDecision) = when (kind) {
    SyncDecision.Run -> tr("Выполнить показанный план?", "Run the displayed plan?")
    SyncDecision.Resume -> tr("Продолжить сохранённый план?", "Resume the saved plan?")
    SyncDecision.Remove -> tr("Убрать пару?", "Remove pair?")
    SyncDecision.Local -> tr("Оставить локальную версию?", "Keep the local version?")
    SyncDecision.Remote -> tr("Оставить облачную версию?", "Keep the cloud version?")
    SyncDecision.Forget -> tr("Принять предупреждение?", "Accept this warning?")
}
@Composable
private fun decisionDescription(kind: SyncDecision) = when (kind) {
    SyncDecision.Run -> tr("Изменения файлов станут обычными заданиями Core. Их состояние и отмена доступны в «Операциях».",
        "File changes become regular Core jobs. Their status and cancellation are available in Operations.")
    SyncDecision.Resume -> tr("Используйте после прерывания или перезапуска. Будет выполнен ранее сохранённый план; уже принятые задания не повторяются. Если плана нет, сначала нажмите «План».",
        "Use after an interruption or restart. The previously saved plan will run; accepted jobs are not repeated. If no plan exists, select Plan first.")
    SyncDecision.Remove -> tr("Пара и её история сравнения будут забыты. Файлы с обеих сторон останутся.",
        "The pair and its comparison history will be forgotten. Files on both sides remain.")
    SyncDecision.Local, SyncDecision.Remote -> tr("Выбор будет применён следующим планом: другая версия будет заменена. После подтверждения заново нажмите «План» и просмотрите изменения.",
        "The next plan applies this choice and replaces the other version. After confirming, select Plan again and inspect the changes.")
    SyncDecision.Forget -> tr("История синхронизации забудет этот путь и всё под ним. Это не удаляет файлы ни локально, ни в облаке. Затем создайте новый план.",
        "Sync history forgets this path and everything below it. This deletes no local or cloud files. Create a new plan afterwards.")
}

@Composable
private fun AddSyncPair(core: CoreGateway, spaces: List<Space>, onClose: () -> Unit,
    onAdd: (Location, Location, Boolean, Boolean) -> Unit) {
    var local by remember { mutableStateOf<Location?>(null) }
    var remote by remember { mutableStateOf<Location?>(null) }
    var twoWay by remember { mutableStateOf(false) }
    var trash by remember { mutableStateOf(false) }
    var choosing by remember { mutableStateOf<Boolean?>(null) }
    AlertDialog(onDismissRequest = onClose, title = { Text(tr("Новая пара", "New pair")) },
        text = { Column(Modifier.heightIn(max = 440.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(tr("Папки должны быть заранее подключены в разделе файлов.", "Connect the folders in Files first."))
            Text(tr("Локально: ", "Local: ") + (local?.let { locationLabel(it, spaces) } ?: tr("Не выбрано", "Not selected")))
            OutlinedButton(onClick = { choosing = false }) { Text(tr("Выбрать локальную папку", "Choose local folder")) }
            Text(tr("Облако: ", "Cloud: ") + (remote?.let { locationLabel(it, spaces) } ?: tr("Не выбрано", "Not selected")))
            OutlinedButton(onClick = { choosing = true }) { Text(tr("Выбрать облачную папку", "Choose cloud folder")) }
            listOf(false, true).forEach { mode ->
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().selectable(twoWay == mode, role = Role.RadioButton,
                    onClick = { twoWay = mode }), verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(twoWay == mode, onClick = null)
                    Text(if (mode) tr("Двусторонняя синхронизация", "Two-way sync") else tr("Бэкап: локально → облако", "Backup: local → cloud"))
                }
            }
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).keyboardFocusOutline().toggleable(trash, role = Role.Checkbox,
                onValueChange = { trash = it }), verticalAlignment = Alignment.CenterVertically) {
                Checkbox(trash, onCheckedChange = null)
                Text(tr("Удалённое локально — в корзину облака", "Local deletions go to cloud trash"))
            }
            Text(tr("Корзина работает только там, где сервер подтвердил её поддержку. Удаления без этой опции показываются как предупреждения.",
                "Trash requires confirmed server support. Without this option, deletions are reported as warnings."), style = MaterialTheme.typography.bodySmall)
        } },
        confirmButton = { TextButton(enabled = local != null && remote != null && local?.storageId != remote?.storageId,
            onClick = { val l = local; val r = remote; if (l != null && r != null) onAdd(l, r, twoWay, trash) }) { Text(tr("Добавить", "Add")) } },
        dismissButton = { TextButton(onClick = onClose) { Text(tr("Отмена", "Cancel")) } })
    choosing?.let { cloud ->
        SyncFolderChooser(core, spaces.filter { it.cloud == cloud }, cloud, onClose = { choosing = null }) { folder ->
            if (cloud) remote = folder else local = folder
            choosing = null
        }
    }
}

/** Selection uses only locations returned by Core, never paths assembled from user input. */
@Composable
private fun SyncFolderChooser(core: CoreGateway, spaces: List<Space>, cloud: Boolean, onClose: () -> Unit, onSelect: (Location) -> Unit) {
    var trail by remember { mutableStateOf<List<Pair<Location, String>>>(emptyList()) }
    var entries by remember { mutableStateOf<List<FileItem>>(emptyList()) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var loading by remember { mutableStateOf(false) }
    val folder = trail.lastOrNull()?.first
    LaunchedEffect(folder) {
        entries = emptyList(); fault = null
        if (folder != null) {
            loading = true
            when (val result = syncSafe { core.list(folder) }) {
                is CoreResult.Value -> entries = result.value.items.filter { it.kind == EntryKind.Directory }
                is CoreResult.Failure -> fault = result.fault
            }
            loading = false
        }
    }
    AlertDialog(onDismissRequest = onClose, title = { Text(if (cloud) tr("Облачная папка", "Cloud folder") else tr("Локальная папка", "Local folder")) },
        text = { Column(Modifier.fillMaxWidth().heightIn(max = 420.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (trail.isNotEmpty()) {
                Text(displayName(trail.joinToString(" / ") { it.second }))
                TextButton(onClick = { trail = trail.dropLast(1) }) { Text(tr("На уровень выше", "Up one level")) }
            }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            fault?.let { FaultNotice(it) }
            if (spaces.isEmpty()) Text(if (cloud) tr("Нет подключённых облаков. Подключите сервер или облако в разделе файлов.",
                "No connected clouds. Connect a server or cloud in Files.") else tr("Нет подключённых локальных папок.", "No connected local folders."))
            LazyColumn(Modifier.weight(1f, fill = false).fillMaxWidth()) {
                if (folder == null) items(spaces, key = { it.id }) { space ->
                    TextButton(onClick = { trail = listOf(space.root to space.name) }, modifier = Modifier.fillMaxWidth()) { Text(displayName(space.name)) }
                } else items(entries, key = { it.location.path }) { entry ->
                    TextButton(onClick = { trail = trail + (entry.location to entry.name) }, modifier = Modifier.fillMaxWidth()) { Text(displayName(entry.name)) }
                }
            }
        } },
        confirmButton = { TextButton(enabled = folder != null && !loading && fault == null,
            onClick = { folder?.let(onSelect) }) { Text(tr("Выбрать эту папку", "Select this folder")) } },
        dismissButton = { TextButton(onClick = onClose) { Text(tr("Отмена", "Cancel")) } })
}
