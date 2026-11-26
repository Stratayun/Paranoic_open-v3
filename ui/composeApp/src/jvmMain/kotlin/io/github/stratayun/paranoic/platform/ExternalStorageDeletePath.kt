package io.github.stratayun.paranoic.platform

/** Strict external-storage document IDs; never infer paths for arbitrary SAF providers. */
internal object ExternalStorageDeletePath {
    data class Id(val volume: String, val parts: List<String>)

    fun part(value: String): Boolean = value.isNotEmpty() && value != "." && value != ".." &&
        value.length <= 255 && value.none { it == '/' || it == '\\' || it == '\u0000' || it == ':' }

    fun parse(id: String): Id {
        require(id.length <= 8192) { "Document ID limit" }
        val colon = id.indexOf(':')
        require(colon > 0) { "Unsupported document ID" }
        val volume = id.substring(0, colon)
        require(part(volume)) { "Invalid volume ID" }
        val path = id.substring(colon + 1)
        val parts = if (path.isEmpty()) emptyList() else path.split('/')
        require(parts.size <= 256 && parts.all(::part)) { "Invalid document path" }
        return Id(volume.lowercase(), parts)
    }

    fun target(treeId: String, documentId: String): Id {
        val root = parse(treeId)
        val document = parse(documentId)
        require(root.volume == document.volume && document.parts.size > root.parts.size &&
            document.parts.take(root.parts.size) == root.parts) { "Document is outside the grant or is its root" }
        return document
    }
}
