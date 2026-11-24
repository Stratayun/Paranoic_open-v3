package io.github.stratayun.paranoic.bridge

/** One-shot input capability. Only platform/Core IO threads call open/read/close.
 * cancel/discard must be non-blocking; no URI or persistent OS grant crosses here. */
interface IncomingByteSource {
    fun read(max: Int): ByteArray
    fun cancel()
    fun close()
}
interface IncomingDocument {
    val id: String
    val name: String
    val size: ULong
    val modifiedMs: ULong?
    fun open(): IncomingByteSource
    fun discard()
}
interface IncomingDocuments {
    val documents: List<IncomingDocument>
    val fault: CoreFault?
    fun take(id: String): IncomingDocument?
    fun discard(id: String)
    fun clear()
}
object NoIncomingDocuments : IncomingDocuments {
    override val documents = emptyList<IncomingDocument>()
    override val fault: CoreFault? = null
    override fun take(id: String): IncomingDocument? = null
    override fun discard(id: String) = Unit
    override fun clear() = Unit
}
