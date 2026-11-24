package io.github.stratayun.paranoic.ui.state

import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

data class DiagnosticPlatformFacts(val platform: String, val version: String, val device: String, val appVersion: String)
interface DiagnosticEnvironment {
    suspend fun facts(): DiagnosticPlatformFacts
    suspend fun previousCrash(): String?
    suspend fun clearCrash()
}
object NoDiagnosticEnvironment : DiagnosticEnvironment {
    override suspend fun facts() = DiagnosticPlatformFacts("Unavailable", "Unavailable", "Unavailable", "Unavailable")
    override suspend fun previousCrash(): String? = null
    override suspend fun clearCrash() = Unit
}
const val MAX_DIAGNOSTIC_CHARACTERS = 262144
data class DiagnosticBundle(val createdAtMs: Long, val text: String) {
    val filename get() = "paranoic-diagnostics-${createdAtMs.coerceAtLeast(0)}.txt"
    init { require(text.length <= MAX_DIAGNOSTIC_CHARACTERS) }
}
fun diagnosticBundle(createdAtMs: Long, facts: DiagnosticPlatformFacts, coreReport: String?,
    recent: List<Diagnostic>, crash: String?): DiagnosticBundle {
    fun field(value: String) = value.filterNot { it.isISOControl() }.take(160)
    val text = buildString {
        appendLine("Paranoic diagnostics")
        appendLine("Platform: ${field(facts.platform)}")
        appendLine("Version: ${field(facts.version)}")
        appendLine("Device: ${field(facts.device)}")
        appendLine("App: ${field(facts.appVersion)}")
        appendLine(); appendLine(coreReport?.take(MAX_DIAGNOSTIC_CHARACTERS) ?: "Core report unavailable")
        appendLine(); appendLine("Recent Core events (shared journal)")
        recent.takeLast(100).forEach { appendLine("${it.atMs}: ${field(it.code)}: ${field(it.message)}") }
        appendLine(); appendLine("Previous crash (exception messages omitted)")
        appendLine(crash ?: "No saved crash")
    }
    val bounded = if (text.length <= MAX_DIAGNOSTIC_CHARACTERS) text else
        text.take(MAX_DIAGNOSTIC_CHARACTERS - 32).dropLastWhile { it.isHighSurrogate() } + "\n[Report truncated]\n"
    return DiagnosticBundle(createdAtMs, bounded)
}

/** Diagnostic cleanup after an explicit Clear/confirmed auto-wipe never repeats a data wipe. */
suspend fun clearDiagnosticRecords(core: CoreGateway, environment: DiagnosticEnvironment): Boolean = withContext(NonCancellable) {
    var cleared = true
    try { if (core.clearDiagnostics() is CoreResult.Failure) cleared = false }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { cleared = false }
    try { environment.clearCrash() }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) { cleared = false }
    cleared
}
