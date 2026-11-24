package io.github.stratayun.paranoic.platform

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import io.github.stratayun.paranoic.nativecore.NativeGrant
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** NoBackup + Keystore protect grant references, never a Private Vault key. */
class AndroidGrantStore private constructor(private val file: AtomicFile, private val alias: String) : NativeGrantStore {
    constructor(context: Context) : this(AtomicFile(File(context.noBackupFilesDir, "grants-v1.enc")), "paranoic-grants-v1")
    /** Fixtures use their own directory and must never access the application's key. */
    internal constructor(directory: File, testAlias: String) : this(AtomicFile(File(directory, "grants-v1.enc")), testAlias) {
        require(testAlias.startsWith("paranoic-test-"))
    }
    private fun exists() = file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()
    private fun key(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        check(create) { "grant key unavailable" }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }
    @Synchronized override fun read(): List<NativeGrant> {
        if (!exists()) return emptyList()
        val data = file.openRead().use { input ->
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                require(output.size() + n <= GrantCodec.MAX_BYTES)
                output.write(buffer, 0, n)
            }
            output.toByteArray()
        }
        require(data.size in 29..GrantCodec.MAX_BYTES)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(128, data.copyOfRange(0, 12)))
        cipher.updateAAD("paranoic/grants/v1".toByteArray())
        val plain = cipher.doFinal(data, 12, data.size - 12)
        return try { GrantCodec.decode(plain) } finally { plain.fill(0) }
    }
    @Synchronized override fun write(grants: List<NativeGrant>) {
        val plain = GrantCodec.encode(grants)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key(!exists()))
        cipher.updateAAD("paranoic/grants/v1".toByteArray())
        val encoded = try { cipher.iv + cipher.doFinal(plain) } finally { plain.fill(0) }
        // Include the IV/tag overhead before opening the atomic replacement.
        require(encoded.size <= GrantCodec.MAX_BYTES) { "Encrypted grant store exceeds byte limit" }
        val output = file.startWrite()
        try { output.write(encoded); file.finishWrite(output) }
        catch (e: Exception) { file.failWrite(output); throw e }
    }
}


