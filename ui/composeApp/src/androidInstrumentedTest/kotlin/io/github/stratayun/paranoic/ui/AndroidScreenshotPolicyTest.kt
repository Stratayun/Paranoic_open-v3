package io.github.stratayun.paranoic.ui

import android.util.AtomicFile
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.stratayun.paranoic.platform.AndroidScreenshotPolicy
import kotlinx.coroutines.runBlocking
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.nio.file.Files
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** Separate disposable files, never the real app preference. Prepared, not run. */
@RunWith(AndroidJUnit4::class)
class AndroidScreenshotPolicyTest {
    private fun fixture(block: (File) -> Unit) {
        val cache = InstrumentationRegistry.getInstrumentation().targetContext.cacheDir
        val directory = Files.createTempDirectory(cache.toPath(), "capture-policy-").toFile()
        try { block(directory) } finally { directory.deleteRecursively() }
    }

    @Test fun missingOrMalformedConsentNeverAllowsCapture() = fixture { directory ->
        val file = File(directory, "policy")
        assertFalse(AndroidScreenshotPolicy(AtomicFile(file)).allowed)
        listOf(byteArrayOf(), byteArrayOf(2), byteArrayOf(-1), byteArrayOf(1, 0)).forEach { bad ->
            file.writeBytes(bad)
            assertFalse(AndroidScreenshotPolicy(AtomicFile(file)).allowed)
        }
    }

    @Test fun consentAndRevocationAreReadByAFreshInstance() = fixture { directory ->
        val file = File(directory, "policy")
        val policy = AndroidScreenshotPolicy(AtomicFile(file))
        var applied: Boolean? = null
        assertTrue(runBlocking { policy.setAllowed(true) { applied = it } })
        assertEquals(true, applied)
        assertTrue(AndroidScreenshotPolicy(AtomicFile(file)).allowed)
        assertTrue(runBlocking { policy.setAllowed(false) { applied = it } })
        assertEquals(false, applied)
        assertFalse(AndroidScreenshotPolicy(AtomicFile(file)).allowed)
    }

    @Test fun failedRevocationWriteStillRestrictsTheCurrentWindow() = fixture { directory ->
        val parent = File(directory, "parent").apply { mkdir() }
        val file = File(parent, "policy").apply { writeBytes(byteArrayOf(1)) }
        val policy = AndroidScreenshotPolicy(AtomicFile(file))
        assertTrue(policy.allowed)
        parent.deleteRecursively()
        parent.writeText("not a directory")
        var applied: Boolean? = null
        assertFalse(runBlocking { policy.setAllowed(false) { applied = it } })
        assertFalse(policy.allowed)
        assertEquals(false, applied)
    }
}
