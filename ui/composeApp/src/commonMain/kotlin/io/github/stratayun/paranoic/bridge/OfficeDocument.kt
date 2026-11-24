package io.github.stratayun.paranoic.bridge

/** Parsed content, not a promise of the original page layout. Never persisted by the UI. */
sealed interface OfficeDocument {
    val truncated: Boolean
    data class Word(val blocks: List<OfficeBlock>, override val truncated: Boolean) : OfficeDocument
    data class Workbook(val sheets: List<OfficeSheet>, override val truncated: Boolean) : OfficeDocument
}

sealed interface OfficeBlock {
    data class Paragraph(val heading: Int?, val list: Boolean, val runs: List<OfficeRun>) : OfficeBlock
    data class Table(val rows: List<List<String>>) : OfficeBlock
}
data class OfficeRun(val text: String, val bold: Boolean, val italic: Boolean, val underline: Boolean)
data class OfficeSheet(val name: String, val rows: List<List<String>>, val truncated: Boolean)
