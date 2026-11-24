package io.github.stratayun.paranoic.platform

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import io.github.stratayun.paranoic.ui.state.VaultCatalogStore
import io.github.stratayun.paranoic.ui.state.PlaceCatalogStore
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

internal fun androidVaultCatalogStore(context: Context): VaultCatalogStore = ProtectedVaultCatalogStore(
    context.noBackupFilesDir.toPath().resolve("vault-catalog-v1.enc"),
    androidCatalogProtection("paranoic-vault-catalog-v1", "paranoic/vault-catalog/v1"))

internal fun androidPlaceCatalogStore(context: Context): PlaceCatalogStore = ProtectedPlaceCatalogStore(
    context.noBackupFilesDir.toPath().resolve("place-catalog-v1.enc"),
    androidCatalogProtection("paranoic-place-catalog-v1", "paranoic/place-catalog/v1"))

private fun androidCatalogProtection(alias: String, purpose: String): CatalogProtection = object : CatalogProtection {
        private fun key(create: Boolean): SecretKey {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (store.getKey(alias, null) as? SecretKey)?.let { return it }
            check(create) { "Catalog key unavailable" }
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
            }.generateKey()
        }
        override fun seal(data: ByteArray, createKey: Boolean): ByteArray {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key(createKey)); cipher.updateAAD(purpose.toByteArray(Charsets.UTF_8))
            return cipher.iv + cipher.doFinal(data)
        }
        override fun open(data: ByteArray): ByteArray {
            require(data.size >= 28)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(128, data.copyOfRange(0, 12)))
            cipher.updateAAD(purpose.toByteArray(Charsets.UTF_8))
            return cipher.doFinal(data, 12, data.size - 12)
        }
    }
