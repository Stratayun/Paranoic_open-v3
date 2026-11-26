package io.github.stratayun.paranoic.platform

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import io.github.stratayun.paranoic.nativecore.HostBytes
import io.github.stratayun.paranoic.nativecore.NotesKeyHost
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Device key of the protected notes (1.9): a non-exportable Keystore
 * AES-256-GCM key, StrongBox when the device has it. It wraps the notes key
 * record together with the password; Clear deletes the live Keystore key.
 * Old key-blob restoration is not ruled out without rollback resistance.
 *
 * Honest limits: deleting a Keystore key is done by the device's TEE/StrongBox;
 * Paranoic cannot verify the hardware actually erased it. A software-only
 * Keystore gives no hardware guarantee (reported as DeviceKey, not HardwareKey).
 * Output: 12-byte IV || ciphertext+tag.
 */
class AndroidNotesKey internal constructor(private val alias: String) : NotesKeyHost {
    constructor() : this("paranoic-notes-v1")
    private val aad = "paranoic/notes/v1".toByteArray()
    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun existing(): SecretKey? = store().getKey(alias, null) as? SecretKey

    @Synchronized private fun keyForSealing(): SecretKey {
        existing()?.let { return it }
        fun generate(strongBox: Boolean): SecretKey = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).setRandomizedEncryptionRequired(true)
            if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) spec.setIsStrongBoxBacked(true)
            // Audit 26.11: unusable while the phone is locked, so tools working on a locked
            // phone cannot use it as an oracle for password guessing. The Safe is locked anyway then.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) spec.setUnlockedDeviceRequired(true)
            init(spec.build())
        }.generateKey()
        return try { generate(strongBox = true) } catch (_: Exception) { generate(strongBox = false) }
    }

    override fun seal(data: ByteArray): HostBytes = try {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, keyForSealing())
        cipher.updateAAD(aad)
        HostBytes(0u, cipher.iv + cipher.doFinal(data))
    } catch (_: Exception) { HostBytes(3u, byteArrayOf()) } finally { data.fill(0) }

    /** Never creates a key: a missing key means the record cannot be opened. */
    override fun open(data: ByteArray): HostBytes {
        try {
            if (data.size <= 12 + 16) return HostBytes(1u, byteArrayOf())
            val key = existing() ?: return HostBytes(1u, byteArrayOf())
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, data.copyOfRange(0, 12)))
            cipher.updateAAD(aad)
            return HostBytes(0u, cipher.doFinal(data, 12, data.size - 12))
        } catch (_: Exception) { return HostBytes(1u, byteArrayOf()) }
    }

    @Synchronized override fun eraseKey(): UInt = try {
        val keys = store()
        if (keys.containsAlias(alias)) keys.deleteEntry(alias)
        if (keys.containsAlias(alias)) 1u else 0u
    } catch (_: Exception) { 1u }

    /** StrongBox backing alone does not attest ROLLBACK_RESISTANCE. The public
     * AES-key API used here supplies no such proof, so never promise final
     * erasure of restored key blobs. 1 = device key, including StrongBox. */
    override fun guarantee(): UByte = 1u
}
