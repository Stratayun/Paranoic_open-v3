package io.github.stratayun.paranoic.bridge

enum class TextEncoding { Utf8, Utf8Bom, Utf16Le, Utf16Be }
/** Opaque version of the encoded bytes. Pass every field back unchanged. */
data class TextFileVersion(val bytes: ULong, val modifiedMs: ULong?, val blake3: String)
data class TextDocument(val text: String, val encoding: TextEncoding, val version: TextFileVersion)
/** Rows may be fragments of long logical lines. A pager never grants write access. */
data class TextPager(val handle: String, val rows: Long, val encoding: TextEncoding, val bytes: ULong)
