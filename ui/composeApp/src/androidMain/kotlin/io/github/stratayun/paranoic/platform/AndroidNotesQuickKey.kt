package io.github.stratayun.paranoic.platform

import android.os.Build
import android.os.SystemClock
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.NotesKeyHost
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Independent from AndroidNotesKey. API 30+ only, no device-credential fallback.
 * The timed wrapping key alone would NOT revoke on enrollment. A separate auth-per-use
 * guard must complete through CryptoObject on every prompt; enrollment invalidates it.
 * Native seal/open additionally consumes a short, one-use in-memory permit.
 */
class AndroidNotesQuickKey internal constructor(private val wrapAlias: String, private val guardAlias: String) : NotesKeyHost {
    constructor() : this("paranoic-notes-quick-wrap-v1", "paranoic-notes-quick-guard-v1")
    private val aad = "paranoic/notes/quick/v1".toByteArray()
    private var generation = 0L
    private var permit: Pair<Boolean, Long>? = null
    internal data class Prepared(val cipher: Cipher, val generation: Long, val enabling: Boolean)
    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun existing(alias: String) = store().getKey(alias, null) as? SecretKey
    private fun generate(alias: String, timeout: Int) {
        check(Build.VERSION.SDK_INT >= 30)
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256).setRandomizedEncryptionRequired(true).setUserAuthenticationRequired(true)
            .setUserAuthenticationParameters(timeout, KeyProperties.AUTH_BIOMETRIC_STRONG)
            .setInvalidatedByBiometricEnrollment(true).setUnlockedDeviceRequired(true).build()
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply { init(spec) }.generateKey()
    }
    private fun guardCipher(): Cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
        init(Cipher.ENCRYPT_MODE, existing(guardAlias) ?: error("quick key missing"))
    }
    @Synchronized internal fun prepare(enabling: Boolean): Prepared {
        revoke()
        check(Build.VERSION.SDK_INT >= 30)
        if (enabling) {
            check(eraseKey() == 0u)
            generate(wrapAlias, 15)
            generate(guardAlias, 0)
        } else check(existing(wrapAlias) != null) // Unlock never creates replacement keys.
        return Prepared(guardCipher(), generation, enabling)
    }
    @Synchronized internal fun authorize(prepared: Prepared, authenticated: Cipher): Boolean {
        if (prepared.generation != generation || authenticated !== prepared.cipher) return false
        return try {
            authenticated.doFinal("paranoic/notes/quick/auth".toByteArray()).fill(0)
            permit = prepared.enabling to (SystemClock.elapsedRealtime() + 15_000)
            true
        } catch (_: Exception) { permit = null; false }
    }
    @Synchronized internal fun revoke() { ++generation; permit = null }
    private fun consume(enabling: Boolean) {
        val allowed = permit
        permit = null
        check(allowed != null && allowed.first == enabling && SystemClock.elapsedRealtime() <= allowed.second)
        // Initialisation also checks that the auth-per-use enrollment guard is still valid.
        guardCipher()
    }
    @Synchronized override fun seal(data: ByteArray): HostBytes = try {
        consume(true)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, existing(wrapAlias) ?: error("quick key missing"))
        cipher.updateAAD(aad)
        HostBytes(0u, cipher.iv + cipher.doFinal(data))
    } catch (_: Exception) { HostBytes(3u, byteArrayOf()) } finally { data.fill(0) }
    @Synchronized override fun open(data: ByteArray): HostBytes = try {
        consume(false)
        check(data.size > 28)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, existing(wrapAlias) ?: error("quick key missing"), GCMParameterSpec(128, data.copyOfRange(0, 12)))
        cipher.updateAAD(aad)
        HostBytes(0u, cipher.doFinal(data, 12, data.size - 12))
    } catch (_: Exception) { HostBytes(1u, byteArrayOf()) }
    @Synchronized override fun eraseKey(): UInt {
        revoke()
        var failed = false
        for (alias in listOf(wrapAlias, guardAlias)) try {
            val keys = store()
            if (keys.containsAlias(alias)) keys.deleteEntry(alias)
            if (keys.containsAlias(alias)) failed = true
        } catch (_: Exception) { failed = true }
        return if (failed) 1u else 0u
    }
    override fun guarantee(): UByte = 1u // No claim of hardware rollback resistance.
}
