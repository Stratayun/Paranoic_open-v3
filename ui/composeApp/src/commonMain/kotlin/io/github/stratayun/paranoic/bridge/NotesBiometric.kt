package io.github.stratayun.paranoic.bridge

/** Platform authentication only; ordinary app-entry authentication cannot unlock notes. */
interface NotesBiometric {
    val available: Boolean
    suspend fun authenticate(enabling: Boolean): Boolean
    fun cancel()
}
object NoNotesBiometric : NotesBiometric {
    override val available = false
    override suspend fun authenticate(enabling: Boolean) = false
    override fun cancel() = Unit
}
