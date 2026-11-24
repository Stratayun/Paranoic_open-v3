package io.github.stratayun.paranoic.ui.state

import androidx.compose.runtime.*
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.*

data class ArchiveState(val source: FileItem? = null, val format: ArchiveFormat = ArchiveFormat.Zip,
    val entries: List<ArchiveEntry> = emptyList(), val loading: Boolean = false,
    val loaded: Boolean = false, val fault: CoreFault? = null)

/** Passwords are command arguments only, never retained in presentation state. */
class ArchiveSession(private val core: CoreGateway, private val scope: CoroutineScope) {
    var state by mutableStateOf(ArchiveState())
        private set
    private var revision = 0L
    private var task: Job? = null
    fun open(source: FileItem, format: ArchiveFormat, password: String?) {
        close()
        val request = ++revision
        state = ArchiveState(source, format, loading = true)
        task = scope.launch {
            val result = try { core.archiveListRequested(source.location, format, password, randomRequestId()) }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { CoreResult.Failure(CoreFault.Unavailable) }
            currentCoroutineContext().ensureActive()
            if (request != revision) return@launch
            state = when (result) {
                is CoreResult.Value -> ArchiveState(source, format, result.value, loaded = true)
                is CoreResult.Failure -> ArchiveState(source, format, fault = result.fault)
            }
        }
    }
    fun close() { ++revision; task?.cancel(); task = null; state = ArchiveState() }
}
