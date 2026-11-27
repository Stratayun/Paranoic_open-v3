package io.github.stratayun.paranoic.ui.screens.documents

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.bridge.*
import io.github.stratayun.paranoic.ui.components.AppIcon
import io.github.stratayun.paranoic.ui.components.AppSymbol
import io.github.stratayun.paranoic.ui.components.FaultNotice
import io.github.stratayun.paranoic.ui.state.*
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/**
 * Markdown opens formatted on the paper page (owner 26.11); "Edit" switches to the plain text editor.
 * Links and images are shown as text only: nothing is fetched or opened from a document.
 */
@Composable
fun MarkdownViewer(core: CoreGateway, file: FileItem, parent: Location?, guard: DocumentExitGuard, onClose: () -> Unit) {
    var editing by remember(file.location) { mutableStateOf(file.bytes > 262_144uL) }
    if (editing) TextEditor(core, file, parent, guard, onClose = { if (file.bytes <= 262_144uL) guard.request { editing = false } else onClose() })
    else MarkdownReader(core, file, onClose, onEdit = { editing = true })
}

@Composable
private fun MarkdownReader(core: CoreGateway, file: FileItem, onClose: () -> Unit, onEdit: () -> Unit) {
    var blocks by remember { mutableStateOf<List<MdBlock>?>(null) }
    var fault by remember { mutableStateOf<CoreFault?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    LaunchedEffect(attempt) {
        blocks = null; fault = null
        val result = core.textOpen(file.location)
        currentCoroutineContext().ensureActive()
        when (result) {
            // Audit A42: the real text size decides (not the listed one), and parsing runs off the UI thread.
            is CoreResult.Value -> if (result.value.text.length > 262_144) onEdit()
                else blocks = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) { parseMarkdown(result.value.text) }
            is CoreResult.Failure -> if (result.fault == CoreFault.Limit) onEdit() else fault = result.fault
        }
    }
    DisposableEffect(Unit) { onDispose { blocks = null } }
    val zoom = remember { LayoutZoom() }
    DocumentFrame(file.name, onClose, controls = {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onEdit) { AppIcon(AppSymbol.Edit, Modifier.size(18.dp)); Spacer(Modifier.width(6.dp)); Text(tr("Редактировать", "Edit")) }
            Spacer(Modifier.weight(1f))
            LayoutZoomControls(zoom)
        }
        fault?.let {
            FaultNotice(it)
            TextButton(onClick = { attempt++ }) { Text(tr("Повторить", "Retry")) }
        }
    }) {
        Box(Modifier.fillMaxSize()) {
            val shown = blocks
            if (shown == null) { if (fault == null) CircularProgressIndicator(Modifier.align(Alignment.Center)) }
            else PaperPage(zoom = zoom) {
                if (shown.isEmpty()) Text(tr("Документ пуст.", "The document is empty."), style = Paper.small)
                LazyColumn(Modifier.fillMaxSize()) {
                    itemsIndexed(shown) { index, block -> MdBlockView(block, index == 0) }
                }
            }
        }
    }
}

internal sealed interface MdBlock {
    data class Heading(val level: Int, val text: String) : MdBlock
    data class Paragraph(val text: String) : MdBlock
    data class Item(val marker: String, val text: String, val depth: Int) : MdBlock
    data class Quote(val text: String) : MdBlock
    data class Code(val text: String) : MdBlock
    data class TableRow(val cells: List<String>, val header: Boolean) : MdBlock
    data object Rule : MdBlock
}

private const val MAX_BLOCKS = 20_000
private val ITEM = Regex("""^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$""")
private val HEADING = Regex("""^(#{1,6})\s+(.*?)\s*#*\s*$""")
private val RULE = Regex("""^\s{0,3}([-*_])(\s*\1){2,}\s*$""")
private val TABLE_SEPARATOR = Regex("""^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$""")

/** Line-based CommonMark subset: headings, paragraphs, lists, task lists, quotes, code fences, rules, pipe tables. */
internal fun parseMarkdown(source: String): List<MdBlock> {
    val out = ArrayList<MdBlock>()
    val lines = source.replace("\r\n", "\n").replace('\r', '\n').split('\n')
    val paragraph = StringBuilder()
    fun flush() { if (paragraph.isNotBlank()) out += MdBlock.Paragraph(paragraph.toString().trim()); paragraph.clear() }
    var i = 0
    while (i < lines.size && out.size < MAX_BLOCKS) {
        val line = lines[i]
        val trimmed = line.trim()
        when {
            trimmed.startsWith("```") || trimmed.startsWith("~~~") -> {
                flush()
                val fence = trimmed.take(3)
                val code = StringBuilder()
                i++
                while (i < lines.size && !lines[i].trim().startsWith(fence)) { if (code.length < 200_000) code.append(lines[i]).append('\n'); i++ }
                out += MdBlock.Code(code.toString().trimEnd('\n'))
            }
            trimmed.isEmpty() -> flush()
            HEADING.matches(trimmed) -> { flush(); HEADING.find(trimmed)!!.let { out += MdBlock.Heading(it.groupValues[1].length, it.groupValues[2]) } }
            RULE.matches(line) -> { flush(); out += MdBlock.Rule }
            trimmed.startsWith(">") -> {
                flush()
                val quote = StringBuilder()
                while (i < lines.size && lines[i].trim().startsWith(">")) { quote.append(lines[i].trim().removePrefix(">").trim()).append(' '); i++ }
                out += MdBlock.Quote(quote.toString().trim()); continue
            }
            trimmed.startsWith("|") && i + 1 < lines.size && TABLE_SEPARATOR.matches(lines[i + 1]) -> {
                flush()
                out += MdBlock.TableRow(cells(trimmed), header = true)
                i += 2
                while (i < lines.size && lines[i].trim().startsWith("|") && out.size < MAX_BLOCKS) { out += MdBlock.TableRow(cells(lines[i].trim()), header = false); i++ }
                continue
            }
            ITEM.matches(line) -> {
                flush()
                val m = ITEM.find(line)!!
                val indent = m.groupValues[1].replace("\t", "    ").length
                var marker = m.groupValues[2]; var text = m.groupValues[3]
                if (marker.first().isDigit()) marker = marker.dropLast(1) + "."
                else marker = "•"
                when {
                    text.startsWith("[ ] ") -> { marker = "☐"; text = text.drop(4) }
                    text.startsWith("[x] ", ignoreCase = true) -> { marker = "☑"; text = text.drop(4) }
                }
                out += MdBlock.Item(marker, text, (indent / 2).coerceAtMost(6))
            }
            else -> {
                // Setext headings: a line of === or --- under the paragraph text.
                val next = lines.getOrNull(i + 1)?.trim()
                if (paragraph.isEmpty() && next != null && next.length >= 2 && (next.all { it == '=' } || next.all { it == '-' })) {
                    out += MdBlock.Heading(if (next.first() == '=') 1 else 2, trimmed); i += 2; continue
                }
                if (paragraph.isNotEmpty()) paragraph.append(if (line.endsWith("  ")) "\n" else " ")
                if (paragraph.length < 200_000) paragraph.append(trimmed)
            }
        }
        i++
    }
    flush()
    return out
}

private fun cells(row: String): List<String> = row.trim().removePrefix("|").removeSuffix("|").split('|').map { it.trim() }.take(64)

/** Inline emphasis, code, strike-through and links (link text only, the address is shown dimmed). */
internal fun inline(text: String): AnnotatedString = buildAnnotatedString {
    var i = 0
    // Audit A42: once a delimiter has no later occurrence, never search for it again
    // (unclosed `*`, `[`… made the scan quadratic).
    val absentFrom = HashMap<String, Int>()
    fun find(what: String, from: Int): Int {
        if (absentFrom[what]?.let { from >= it } == true) return -1
        val at = text.indexOf(what, from)
        if (at < 0) absentFrom[what] = minOf(absentFrom[what] ?: Int.MAX_VALUE, from)
        return at
    }
    // snake_case words are not emphasis: "_" opens only at a word start.
    fun wordStart(at: Int) = at == 0 || !text[at - 1].isLetterOrDigit()
    fun styled(open: String, style: SpanStyle): Boolean {
        if (!text.startsWith(open, i)) return false
        val end = find(open, i + open.length)
        if (end <= i + open.length) return false
        withStyle(style) { append(inline(text.substring(i + open.length, end))) }
        i = end + open.length
        return true
    }
    while (i < text.length) {
        val c = text[i]
        when {
            c == '\\' && i + 1 < text.length -> { append(text[i + 1]); i += 2 }
            c == '`' -> {
                val end = find("`", i + 1)
                if (end > i) { withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = Paper.code, fontSize = 13.sp)) { append(text.substring(i + 1, end)) }; i = end + 1 }
                else { append(c); i++ }
            }
            styled("**", SpanStyle(fontWeight = FontWeight.Bold)) -> Unit
            wordStart(i) && styled("__", SpanStyle(fontWeight = FontWeight.Bold)) -> Unit
            styled("~~", SpanStyle(textDecoration = TextDecoration.LineThrough)) -> Unit
            (c == '*' || (c == '_' && wordStart(i))) && i + 1 < text.length && text[i + 1] != ' ' && styled(c.toString(), SpanStyle(fontStyle = FontStyle.Italic)) -> Unit
            (c == '[' || (c == '!' && text.startsWith("![", i))) -> {
                val image = c == '!'
                val open = if (image) i + 1 else i
                val close = find("](", open)
                val end = if (close > 0) find(")", close) else -1
                if (close > open && end > close) {
                    val label = text.substring(open + 1, close)
                    if (image) withStyle(SpanStyle(color = Paper.muted, fontStyle = FontStyle.Italic)) { append("[" + label.ifEmpty { "image" } + "]") }
                    else withStyle(SpanStyle(color = Paper.accent, textDecoration = TextDecoration.Underline)) { append(inline(label)) }
                    i = end + 1
                } else { append(c); i++ }
            }
            else -> { append(c); i++ }
        }
    }
}

@Composable
private fun MdBlockView(block: MdBlock, first: Boolean) {
    when (block) {
        is MdBlock.Heading -> Column(Modifier.padding(top = if (first) 0.dp else 16.dp, bottom = 6.dp).sectionHeading()) {
            SelectionContainer { Text(inline(block.text), style = Paper.heading(block.level)) }
            if (block.level <= 2) HorizontalDivider(Modifier.padding(top = 4.dp), color = Paper.rule)
        }
        is MdBlock.Paragraph -> SelectionContainer { Text(inline(block.text), Modifier.padding(bottom = 10.dp), style = Paper.body) }
        is MdBlock.Item -> Row(Modifier.padding(start = (8 + block.depth * 16).dp, bottom = 4.dp)) {
            Text(block.marker, Modifier.widthIn(min = 22.dp), style = Paper.body.copy(color = Paper.accent))
            SelectionContainer { Text(inline(block.text), style = Paper.body) }
        }
        is MdBlock.Quote -> Row(Modifier.padding(bottom = 10.dp).height(IntrinsicSize.Min)) {
            Box(Modifier.width(3.dp).fillMaxHeight().background(Paper.quote))
            SelectionContainer { Text(inline(block.text), Modifier.padding(start = 12.dp), style = Paper.body.copy(color = Paper.muted, fontStyle = FontStyle.Italic)) }
        }
        is MdBlock.Code -> Box(Modifier.fillMaxWidth().padding(bottom = 10.dp).background(Paper.code, MaterialTheme.shapes.small)
            .horizontalScroll(rememberScrollState()).padding(12.dp)) {
            SelectionContainer { Text(block.text, style = Paper.body.copy(fontFamily = FontFamily.Monospace, fontSize = 13.sp, lineHeight = 19.sp)) }
        }
        is MdBlock.TableRow -> Row(Modifier.horizontalScroll(rememberScrollState())) {
            block.cells.forEach { cell ->
                Box(Modifier.width(140.dp).heightIn(min = 36.dp).background(if (block.header) Paper.header else Paper.sheet)
                    .border(0.5.dp, Paper.rule).padding(8.dp)) {
                    Text(inline(cell), style = if (block.header) Paper.body.copy(fontSize = 13.sp, fontWeight = FontWeight.SemiBold) else Paper.body.copy(fontSize = 13.sp, lineHeight = 18.sp))
                }
            }
        }
        MdBlock.Rule -> HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Paper.rule)
    }
}
