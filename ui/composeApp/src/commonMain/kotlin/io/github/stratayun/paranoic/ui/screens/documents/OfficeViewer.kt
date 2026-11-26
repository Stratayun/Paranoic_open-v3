package io.github.stratayun.paranoic.ui.screens.documents

import io.github.stratayun.paranoic.ui.components.FaultNotice
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.ui.components.AppIcon
import io.github.stratayun.paranoic.ui.components.AppSymbol
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
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(tr("Только просмотр", "Read only"), Modifier.weight(1f).padding(start = 12.dp),
                style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            ZoomControls(zoom)
        }
        fault?.let {
            FaultNotice(it)
            TextButton(onClick = { attempt++ }) { Text(tr("Повторить", "Retry")) }
        }
        if (opened?.truncated == true) LimitNotice()
    }) {
        ZoomableDocument(zoom) {
            when (opened) {
                is OfficeDocument.Word -> PaperPage { WordContent(opened, Modifier.fillMaxSize()) }
                is OfficeDocument.Workbook -> PaperPage(padding = PaddingValues(0.dp)) { WorkbookContent(opened, Modifier.fillMaxSize()) }
                null -> if (fault == null) CircularProgressIndicator(Modifier.align(Alignment.Center))
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
    data class TableRow(val cells: List<String>, val first: Boolean) : WordLine
}
private data class SelectedCell(val title: String, val text: String)

@Composable
private fun WordContent(document: OfficeDocument.Word, modifier: Modifier) {
    val lines = remember(document) {
        buildList {
            document.blocks.forEach { block ->
                when (block) {
                    is OfficeBlock.Paragraph -> add(WordLine.Paragraph(block))
                    is OfficeBlock.Table -> block.rows.forEachIndexed { i, row -> add(WordLine.TableRow(row, i == 0)) }
                }
            }
        }
    }
    var selected by remember { mutableStateOf<SelectedCell?>(null) }
    val cellTitle = tr("Содержимое ячейки", "Cell contents")
    if (lines.isEmpty()) Text(tr("Документ пуст.", "The document is empty."), style = Paper.small)
    LazyColumn(modifier.fillMaxWidth()) {
        itemsIndexed(lines) { index, line ->
            when (line) {
                is WordLine.Paragraph -> WordParagraph(line.value, index == 0)
                is WordLine.TableRow -> Row(Modifier.padding(top = if (line.first) 8.dp else 0.dp).horizontalScroll(rememberScrollState())) {
                    line.cells.forEach { value -> GridCell(value, 150.dp, header = line.first) { selected = SelectedCell(cellTitle, value) } }
                }
            }
        }
    }
    selected?.let { CellDialog(it) { selected = null } }
}

@Composable
private fun WordParagraph(paragraph: OfficeBlock.Paragraph, first: Boolean) {
    val text = remember(paragraph) {
        buildAnnotatedString {
            if (paragraph.list) append("•  ")
            paragraph.runs.forEach { run ->
                withStyle(SpanStyle(fontWeight = if (run.bold) FontWeight.Bold else null,
                    fontStyle = if (run.italic) FontStyle.Italic else FontStyle.Normal,
                    textDecoration = if (run.underline) TextDecoration.Underline else TextDecoration.None)) {
                    append(run.text)
                }
            }
        }
    }
    val heading = paragraph.heading
    val top = when { first -> 0.dp; heading != null -> 14.dp; else -> 0.dp }
    SelectionContainer {
        Text(text, modifier = (if (heading != null) Modifier.sectionHeading() else Modifier)
            .padding(top = top, bottom = if (heading != null) 6.dp else 10.dp, start = if (paragraph.list) 12.dp else 0.dp),
            style = if (heading != null) Paper.heading(heading) else Paper.body)
    }
}

@Composable
private fun WorkbookContent(document: OfficeDocument.Workbook, modifier: Modifier) {
    var sheetIndex by remember(document) { mutableIntStateOf(0) }
    var selected by remember(document) { mutableStateOf<SelectedCell?>(null) }
    val sheet = document.sheets.getOrNull(sheetIndex)
    Column(modifier.fillMaxWidth()) {
        if (sheet == null) Text(tr("В книге нет листов.", "The workbook has no sheets."), Modifier.padding(16.dp), style = Paper.small)
        else {
            if (sheet.truncated && !document.truncated) Box(Modifier.padding(8.dp)) { LimitNotice() }
            // Reset both scroll axes and column page when changing sheets.
            key(sheetIndex) { SheetContent(sheet, Modifier.weight(1f)) { selected = it } }
            // Sheet tabs along the bottom edge, as in a spreadsheet program.
            HorizontalDivider(color = Paper.rule)
            Row(Modifier.fillMaxWidth().background(Paper.header).horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp)) {
                document.sheets.forEachIndexed { index, item ->
                    val on = index == sheetIndex
                    Box(Modifier.clickable(role = Role.Tab, onClickLabel = displayName(item.name)) { sheetIndex = index; selected = null }
                        .background(if (on) Paper.sheet else Color.Transparent)
                        .padding(horizontal = 14.dp, vertical = 10.dp)) {
                        Text(displayName(item.name), maxLines = 1, overflow = TextOverflow.Ellipsis,
                            style = if (on) Paper.body.copy(color = Paper.accent, fontWeight = FontWeight.SemiBold, fontSize = 14.sp) else Paper.small.copy(fontSize = 14.sp))
                    }
                }
            }
        }
    }
    selected?.let { CellDialog(it) { selected = null } }
}

private const val COLUMNS_PER_PAGE = 8
private val ROW_HEAD = 48.dp
private val CELL = 124.dp

@Composable
private fun SheetContent(sheet: OfficeSheet, modifier: Modifier, onSelect: (SelectedCell) -> Unit) {
    var firstColumn by remember { mutableIntStateOf(0) }
    val columns = remember(sheet) { sheet.rows.maxOfOrNull { it.size } ?: 0 }
    val endColumn = minOf(firstColumn + COLUMNS_PER_PAGE, columns)
    val scroll = rememberScrollState()
    Column(modifier.fillMaxWidth()) {
        if (columns == 0) Text(tr("Лист пуст.", "The sheet is empty."), Modifier.padding(16.dp), style = Paper.small)
        else {
            if (columns > COLUMNS_PER_PAGE) Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(enabled = firstColumn > 0, onClick = { firstColumn -= COLUMNS_PER_PAGE },
                    modifier = Modifier.actionDescription(tr("Предыдущие столбцы", "Previous columns"))) { AppIcon(AppSymbol.Prev) }
                Text(tr("Столбцы ", "Columns ") + columnName(firstColumn) + "–" + columnName(endColumn - 1) + tr(" из ", " of ") + columns,
                    style = Paper.small, modifier = Modifier.weight(1f), textAlign = TextAlign.Center)
                IconButton(enabled = endColumn < columns, onClick = { firstColumn += COLUMNS_PER_PAGE },
                    modifier = Modifier.actionDescription(tr("Следующие столбцы", "Next columns"))) { AppIcon(AppSymbol.Forward) }
            }
            // Only eight columns per page are measured; every parsed column remains reachable.
            Column(Modifier.weight(1f).horizontalScroll(scroll)) {
                Row {
                    GridCell("", ROW_HEAD, header = true)
                    for (column in firstColumn until endColumn) GridCell(columnName(column), CELL, header = true, center = true)
                }
                LazyColumn(Modifier.weight(1f).width(ROW_HEAD + CELL * (endColumn - firstColumn))) {
                    itemsIndexed(sheet.rows) { rowIndex, row ->
                        Row {
                            GridCell((rowIndex + 1).toString(), ROW_HEAD, header = true, center = true)
                            for (column in firstColumn until endColumn) {
                                val value = row.getOrElse(column) { "" }
                                val coordinate = columnName(column) + (rowIndex + 1)
                                GridCell(value, CELL, coordinate = coordinate) { onSelect(SelectedCell(coordinate, value)) }
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

/** One spreadsheet-like cell on the paper: thin grid lines, header cells tinted. */
@Composable
private fun GridCell(value: String, width: Dp, header: Boolean = false, center: Boolean = false, coordinate: String? = null, onClick: (() -> Unit)? = null) {
    val label = tr("Показать ячейку полностью", "Show full cell") + (coordinate?.let { " " + it } ?: "")
    var modifier = Modifier.width(width).heightIn(min = 36.dp).background(if (header) Paper.header else Paper.sheet).border(0.5.dp, Paper.rule)
    if (onClick != null && value.isNotEmpty()) modifier = modifier.keyboardFocusOutline().clickable(role = Role.Button, onClickLabel = label, onClick = onClick)
    Box(modifier.padding(horizontal = 8.dp, vertical = 8.dp), contentAlignment = if (center) Alignment.Center else Alignment.CenterStart) {
        // Limit layout work, not access to the value: tapping reveals the full parsed cell.
        Text(value.take(512) + if (value.length > 512) "…" else "", maxLines = if (header) 1 else 2, overflow = TextOverflow.Ellipsis,
            style = if (header) Paper.small.copy(fontWeight = FontWeight.SemiBold) else Paper.body.copy(fontSize = 13.sp, lineHeight = 18.sp))
    }
}

@Composable
private fun CellDialog(cell: SelectedCell, onClose: () -> Unit) {
    AlertDialog(onDismissRequest = onClose, title = { Text(cell.title) },
        text = { SelectionContainer { Text(cell.text, Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) } },
        confirmButton = { TextButton(onClick = onClose) { Text(tr("Закрыть", "Close")) } })
}
