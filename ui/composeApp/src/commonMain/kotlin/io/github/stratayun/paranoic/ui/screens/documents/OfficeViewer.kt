package io.github.stratayun.paranoic.ui.screens.documents

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** Structured read-only view: no external content, macros, temporary files or save operation. */
@Composable
fun OfficeViewer(core: CoreGateway, file: FileItem, onClose: () -> Unit) {
    // Key the whole content, including sheet/cell selection, to this open document.
    key(core, file.location) { OfficeContent(core, file, onClose) }
}

@Composable
private fun OfficeContent(core: CoreGateway, file: FileItem, onClose: () -> Unit) {
    var document by remember { mutableStateOf<OfficeDocument?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    LaunchedEffect(attempt) {
        document = null
        fault = null
        val result = core.officeOpen(file.location)
        currentCoroutineContext().ensureActive()
        when (result) {
            is CoreResult.Value -> document = result.value
            is CoreResult.Failure -> fault = result.fault
        }
    }
    DisposableEffect(Unit) { onDispose { document = null } }
    val opened = document
    val zoom = remember { DocumentZoom() }
    DocumentFrame(file.name, onClose, controls = {
        Text(tr("Только просмотр · структура документа без исходной вёрстки",
            "Read only · document structure without the original layout"), style = MaterialTheme.typography.bodySmall)
        ZoomControls(zoom)
        fault?.let {
            FaultNotice(it)
            TextButton(onClick = { attempt++ }) { Text(tr("Повторить", "Retry")) }
        }
        if (opened?.truncated == true) LimitNotice()
    }) {
        ZoomableDocument(zoom) {
            Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                when (opened) {
                    is OfficeDocument.Word -> WordContent(opened, Modifier.weight(1f))
                    is OfficeDocument.Workbook -> WorkbookContent(opened, Modifier.weight(1f))
                    null -> if (fault == null) CircularProgressIndicator()
                }
            }
        }
    }
}

@Composable
private fun LimitNotice() {
    Text(tr("Достигнут предел просмотра: показана только часть содержимого.",
        "Preview limit reached: only part of the content is shown."), color = MaterialTheme.colorScheme.error,
        style = MaterialTheme.typography.bodySmall)
}

// Tables become individual lazy rows, so a large DOCX table cannot eagerly lay out the whole document.
private sealed interface WordLine {
    data class Paragraph(val value: OfficeBlock.Paragraph) : WordLine
    data class TableRow(val cells: List<String>) : WordLine
}
private data class SelectedCell(val title: String, val text: String)

@Composable
private fun WordContent(document: OfficeDocument.Word, modifier: Modifier) {
    val lines = remember(document) {
        buildList {
            document.blocks.forEach { block ->
                when (block) {
                    is OfficeBlock.Paragraph -> add(WordLine.Paragraph(block))
                    is OfficeBlock.Table -> block.rows.forEach { add(WordLine.TableRow(it)) }
                }
            }
        }
    }
    var selected by remember { mutableStateOf<SelectedCell?>(null) }
    val cellTitle = tr("Содержимое ячейки", "Cell contents")
    if (lines.isEmpty()) Text(tr("Документ пуст.", "The document is empty."))
    LazyColumn(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        itemsIndexed(lines) { _, line ->
            when (line) {
                is WordLine.Paragraph -> WordParagraph(line.value)
                is WordLine.TableRow -> Row(Modifier.horizontalScroll(rememberScrollState())) {
                    line.cells.forEach { value -> OfficeCell(value) { selected = SelectedCell(cellTitle, value) } }
                }
            }
        }
    }
    selected?.let { CellDialog(it) { selected = null } }
}

@Composable
private fun WordParagraph(paragraph: OfficeBlock.Paragraph) {
    val text = remember(paragraph) {
        buildAnnotatedString {
            if (paragraph.list) append("• ")
            paragraph.runs.forEach { run ->
                withStyle(SpanStyle(fontWeight = if (run.bold) FontWeight.Bold else FontWeight.Normal,
                    fontStyle = if (run.italic) FontStyle.Italic else FontStyle.Normal,
                    textDecoration = if (run.underline) TextDecoration.Underline else TextDecoration.None)) {
                    append(run.text)
                }
            }
        }
    }
    val style = when (paragraph.heading ?: -1) {
        0, 1 -> MaterialTheme.typography.headlineSmall
        2 -> MaterialTheme.typography.titleLarge
        in 3..9 -> MaterialTheme.typography.titleMedium
        else -> MaterialTheme.typography.bodyMedium
    }
    SelectionContainer { Text(text, modifier = if (paragraph.heading != null) Modifier.sectionHeading() else Modifier, style = style) }
}

@Composable
private fun WorkbookContent(document: OfficeDocument.Workbook, modifier: Modifier) {
    var sheetIndex by remember(document) { mutableIntStateOf(0) }
    var menu by remember { mutableStateOf(false) }
    var selected by remember(document) { mutableStateOf<SelectedCell?>(null) }
    val sheet = document.sheets.getOrNull(sheetIndex)
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (sheet == null) Text(tr("В книге нет листов.", "The workbook has no sheets."))
        else {
            Box {
                TextButton(onClick = { menu = true }) {
                    Text(tr("Лист: ", "Sheet: ") + displayName(sheet.name), maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    document.sheets.forEachIndexed { index, item ->
                        DropdownMenuItem(text = { Text(displayName(item.name), maxLines = 2) },
                            onClick = { sheetIndex = index; menu = false; selected = null })
                    }
                }
            }
            if (sheet.truncated && !document.truncated) LimitNotice()
            // Reset both scroll axes and column page when changing sheets.
            key(sheetIndex) { SheetContent(sheet, Modifier.weight(1f)) { selected = it } }
        }
    }
    selected?.let { CellDialog(it) { selected = null } }
}

private const val COLUMNS_PER_PAGE = 8

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SheetContent(sheet: OfficeSheet, modifier: Modifier, onSelect: (SelectedCell) -> Unit) {
    var firstColumn by remember { mutableIntStateOf(0) }
    val columns = remember(sheet) { sheet.rows.maxOfOrNull { it.size } ?: 0 }
    val endColumn = minOf(firstColumn + COLUMNS_PER_PAGE, columns)
    val scroll = rememberScrollState()
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (columns == 0) Text(tr("Лист пуст.", "The sheet is empty."))
        else {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(enabled = firstColumn > 0, onClick = { firstColumn -= COLUMNS_PER_PAGE }) {
                    Text(tr("Назад", "Previous"))
                }
                Text(tr("Столбцы ", "Columns ") + columnName(firstColumn) + "–" + columnName(endColumn - 1),
                    Modifier.padding(vertical = 12.dp))
                TextButton(enabled = endColumn < columns, onClick = { firstColumn += COLUMNS_PER_PAGE }) {
                    Text(tr("Далее", "Next"))
                }
            }
            // Only eight columns per page are measured; every parsed column remains reachable.
            Column(Modifier.weight(1f).horizontalScroll(scroll)) {
                Row {
                    Text(tr("Строка", "Row"), Modifier.width(64.dp).padding(8.dp))
                    for (column in firstColumn until endColumn) {
                        Text(columnName(column), Modifier.width(160.dp).padding(8.dp), fontWeight = FontWeight.Bold)
                    }
                }
                LazyColumn(Modifier.weight(1f).width((64 + (endColumn - firstColumn) * 160).dp)) {
                    itemsIndexed(sheet.rows) { rowIndex, row ->
                        Row {
                            Text((rowIndex + 1).toString(), Modifier.width(64.dp).padding(8.dp))
                            for (column in firstColumn until endColumn) {
                                val value = row.getOrElse(column) { "" }
                                OfficeCell(value, columnName(column) + (rowIndex + 1)) { onSelect(SelectedCell(columnName(column) + (rowIndex + 1), value)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** Spreadsheet column label, zero-based: A ... Z, AA ... */
private fun columnName(index: Int): String {
    var number = index + 1
    val result = StringBuilder()
    while (number > 0) {
        number--
        result.append(('A'.code + number % 26).toChar())
        number /= 26
    }
    return result.reverse().toString()
}

@Composable
private fun OfficeCell(value: String, coordinate: String? = null, onClick: () -> Unit) {
    val label = tr("Показать ячейку полностью", "Show full cell") + (coordinate?.let { " " + it } ?: "")
    Surface(Modifier.width(160.dp).heightIn(min = 48.dp).padding(1.dp).keyboardFocusOutline().clickable(role = Role.Button, onClickLabel = label, onClick = onClick),
        color = MaterialTheme.colorScheme.surfaceVariant) {
        // Limit layout work, not access to the value: tapping reveals the full parsed cell.
        Text(value.take(512) + if (value.length > 512) "…" else "", Modifier.padding(8.dp), maxLines = 3, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun CellDialog(cell: SelectedCell, onClose: () -> Unit) {
    AlertDialog(onDismissRequest = onClose, title = { Text(cell.title) },
        text = { SelectionContainer { Text(cell.text, Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) } },
        confirmButton = { TextButton(onClick = onClose) { Text(tr("Закрыть", "Close")) } })
}
