package io.github.stratayun.paranoic.platform

import android.os.Build
import androidx.biometric.BiometricManager
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID
import kotlin.test.*

/** Uses unique aliases. No test deletes live notes/app-entry keys or claims a biometric match. */
@RunWith(AndroidJUnit4::class)
class AndroidNotesQuickKeyTest {
    private fun alias() = "paranoic-test-quick-" + UUID.randomUUID()
    private fun keys() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    @Test fun callbacksWithoutAPromptPermitFailClosedAndSealClearsInput() {
        val wrap = alias(); val guard = alias(); val host = AndroidNotesQuickKey(wrap, guard)
        val data = ByteArray(32) { 7 }
        assertNotEquals(0u, host.seal(data).error)
        assertTrue(data.all { it == 0.toByte() })
        assertNotEquals(0u, host.open(ByteArray(64)).error)
        assertFalse(keys().containsAlias(wrap)); assertFalse(keys().containsAlias(guard))
    }
    @Test fun unlockingNeverRegeneratesMissingKeys() {
        assumeTrue(Build.VERSION.SDK_INT >= 30)
        val wrap = alias(); val guard = alias(); val host = AndroidNotesQuickKey(wrap, guard)
        assertFails { host.prepare(enabling = false) }
        assertFalse(keys().containsAlias(wrap)); assertFalse(keys().containsAlias(guard))
    }
    @Test fun revokeRejectsALateAuthenticationAndClearErasesBothIndependentKeys() {
        assumeTrue(Build.VERSION.SDK_INT >= 30)
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        assumeTrue(BiometricManager.from(context).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS)
        val wrap = alias(); val guard = alias(); val host = AndroidNotesQuickKey(wrap, guard)
        try {
            val prepared = host.prepare(enabling = true)
            assertTrue(keys().containsAlias(wrap)); assertTrue(keys().containsAlias(guard))
            host.revoke()
            assertFalse(host.authorize(prepared, prepared.cipher))
            assertEquals(0u, host.eraseKey())
            assertFalse(keys().containsAlias(wrap)); assertFalse(keys().containsAlias(guard))
            assertFalse(host.authorize(prepared, prepared.cipher), "late callbacks after Clear cannot issue a permit")
        } finally { host.eraseKey() }
    }
}
