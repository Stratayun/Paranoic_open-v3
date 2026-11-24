package io.github.stratayun.paranoic.ui.state

enum class DirectWriteKind { EncryptFile, DecryptFile, CreateArchive, ExtractArchive, CreateText }
enum class DirectWritePhase { Running, Done, Failed, Unknown }
/** No credential, callback or location is retained for retry. Archive IDs also identify publication jobs. */
data class DirectWriteState(val kind: DirectWriteKind, val phase: DirectWritePhase,
    val requestId: String? = null, val cancelling: Boolean = false)
