package io.github.stratayun.paranoic.platform

import com.sun.jna.platform.win32.Crypt32Util
import io.github.stratayun.paranoic.ui.state.VaultCatalogStore
import io.github.stratayun.paranoic.ui.state.PlaceCatalogStore
import java.nio.file.Path

internal fun desktopVaultCatalogStore(root: Path): VaultCatalogStore = ProtectedVaultCatalogStore(
    root.resolve("vault-catalog-v1.dpapi"), desktopCatalogProtection())

internal fun desktopPlaceCatalogStore(root: Path): PlaceCatalogStore = ProtectedPlaceCatalogStore(
    root.resolve("place-catalog-v1.dpapi"), desktopCatalogProtection())

private fun desktopCatalogProtection(): CatalogProtection = object : CatalogProtection {
        private fun windows() { check(System.getProperty("os.name").startsWith("Windows")) { "Protected catalog requires Windows DPAPI" } }
        override fun seal(data: ByteArray, createKey: Boolean): ByteArray { windows(); return Crypt32Util.cryptProtectData(data) }
        override fun open(data: ByteArray): ByteArray { windows(); return Crypt32Util.cryptUnprotectData(data) }
    }
