package io.github.stratayun.paranoic.platform

import io.github.stratayun.paranoic.ui.state.*
import io.github.stratayun.paranoic.bridge.*
import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import kotlin.test.*

class DiagnosticEnvironmentTest {
    private val facts = DiagnosticPlatformFacts("Test", "1", "Public model", "2.1")
    @Test fun crashRecordOmitsMessagesAndSourceFileNames() {
        val secret = "PRIVATE_PASSWORD_AND_FILENAME"
        val error = IllegalStateException("$secret C:\\Users\\private\\vault.txt", IllegalArgumentException(secret))
        error.stackTrace = arrayOf(StackTraceElement("io.github.App", "open", "$secret.kt", 42))
        val bytes = JvmDiagnosticEnvironment.encodeCrash(error, 123)
        val text = JvmDiagnosticEnvironment.decodeCrash(bytes)
        assertFalse(text.contains(secret)); assertFalse(bytes.toString(Charsets.ISO_8859_1).contains(secret))
        assertTrue(text.contains("java.lang.IllegalStateException"))
        assertTrue(text.contains("io.github.App.open:42"))
        assertFalse(text.contains("vault.txt"))
    }
    @Test fun causeCyclesAndLargeStacksRemainBounded() {
        val a = IllegalStateException(); val b = IllegalArgumentException()
        a.initCause(b); b.initCause(a)
        a.stackTrace = Array(200) { StackTraceElement("a".repeat(500), "m".repeat(500), "private", it) }
        val bytes = JvmDiagnosticEnvironment.encodeCrash(a, 10)
        assertTrue(bytes.size <= 32768)
        val text = JvmDiagnosticEnvironment.decodeCrash(bytes)
        assertEquals(2, text.lineSequence().count { it.startsWith("Exception class:") })
        assertTrue(text.contains("unavailable"))
    }
    @Test fun invalidOrTrailingPayloadIsNotAcceptedAsAValidCrash() {
        val good = JvmDiagnosticEnvironment.encodeCrash(IllegalStateException(), 5)
        assertFails { JvmDiagnosticEnvironment.decodeCrash(good + byteArrayOf(1)) }
        assertFails { JvmDiagnosticEnvironment.decodeCrash(byteArrayOf(1, 2, 3)) }
        assertFails { JvmDiagnosticEnvironment.decodeCrash(ByteArray(32769)) }
    }
    @Test fun recordReadAndExplicitClearUseOnlyTheJournalFile() = runBlocking {
        val dir = Files.createTempDirectory("paranoic-diagnostics-"); val file = dir.resolve("crash")
        try {
            val environment = JvmDiagnosticEnvironment(file) { facts }
            assertNull(environment.previousCrash())
            environment.record(IllegalStateException("message must not be stored"))
            assertFalse(assertNotNull(environment.previousCrash()).contains("message must not be stored"))
            assertEquals(facts, environment.facts())
            assertEquals(1L, Files.list(dir).use { it.count() })
            environment.clearCrash(); assertNull(environment.previousCrash())
        } finally { Files.deleteIfExists(file); Files.deleteIfExists(dir) }
    }
    @Test fun corruptJournalIsPreservedUntilExplicitClear() = runBlocking {
        val dir = Files.createTempDirectory("paranoic-diagnostics-corrupt-"); val file = dir.resolve("crash")
        try {
            val bytes = byteArrayOf(1, 2, 3); Files.write(file, bytes)
            val environment = JvmDiagnosticEnvironment(file) { facts }
            assertFails { environment.previousCrash() }
            assertContentEquals(bytes, Files.readAllBytes(file))
        } finally { Files.deleteIfExists(file); Files.deleteIfExists(dir) }
    }
    @Test fun reportIsBoundedAndHasAnIndependentCreateNewName() {
        val report = diagnosticBundle(123, facts.copy(device = "Model\ncontrol"), "x".repeat(300000), emptyList(), null)
        assertTrue(report.text.length <= MAX_DIAGNOSTIC_CHARACTERS)
        assertTrue(report.text.contains("[Report truncated]"))
        assertTrue(report.text.contains("Device: Modelcontrol"))
        assertEquals("paranoic-diagnostics-123.txt", report.filename)
        assertTrue(validNewTextName(report.filename))
    }
    @Test fun diagnosticCleanupDoesNotRepeatANotesWipe() = runBlocking {
        var journalCleared = false; var crashCleared = false
        val core = object : CoreGateway by DisconnectedCoreGateway {
            override suspend fun clearDiagnostics(): CoreResult<Unit> { journalCleared = true; return CoreResult.Value(Unit) }
            override suspend fun notesClear(): CoreResult<KeyGuarantee> = error("Data wipe must not be called")
        }
        val environment = object : DiagnosticEnvironment {
            override suspend fun facts() = facts
            override suspend fun previousCrash(): String? = null
            override suspend fun clearCrash() { crashCleared = true }
        }
        assertTrue(clearDiagnosticRecords(core, environment))
        assertTrue(journalCleared && crashCleared)
    }
    @Test fun diagnosticCleanupTriesBothIndependentStoresAndReportsFailure() = runBlocking {
        var crashAttempted = false
        val environment = object : DiagnosticEnvironment {
            override suspend fun facts() = facts
            override suspend fun previousCrash(): String? = null
            override suspend fun clearCrash() { crashAttempted = true }
        }
        assertFalse(clearDiagnosticRecords(DisconnectedCoreGateway, environment))
        assertTrue(crashAttempted)
    }
}
