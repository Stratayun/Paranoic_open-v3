package io.github.stratayun.paranoic.platform

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.RecordSealer
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Seals Core journal records (paths, names) with a non-exportable Keystore
 * AES-256-GCM key. Output: 12-byte IV || ciphertext+tag. Losing the key (e.g.
 * Keystore reset) makes the old journal unreadable; Core then reports it.
 */
class AndroidRecordSealer : RecordSealer {
    private val alias = "paranoic-journal-v1"
    private val aad = "paranoic/journal/v1".toByteArray()
    @Synchronized private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }
    override fun available() = true
    /** The Keystore key exists already: the journal was sealed before (audit 17). A file writer cannot fake this. */
    override fun established() = try { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.containsAlias(alias) } catch (_: Exception) { false }
    override fun seal(data: ByteArray): HostBytes = try {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        HostBytes(0u, cipher.iv + cipher.doFinal(data))
    } catch (_: Exception) { HostBytes(3u, byteArrayOf()) } finally { data.fill(0) }
    override fun open(data: ByteArray): HostBytes = try {
        require(data.size > 12 + 16)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, data.copyOfRange(0, 12)))
        cipher.updateAAD(aad)
        HostBytes(0u, cipher.doFinal(data, 12, data.size - 12))
    } catch (_: Exception) { HostBytes(1u, byteArrayOf()) }
}
