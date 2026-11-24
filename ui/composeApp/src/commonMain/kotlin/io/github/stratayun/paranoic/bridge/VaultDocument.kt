package io.github.stratayun.paranoic.bridge

/** The copy is verified even when the storage cannot conditionally remove its source. */
data class VaultMoveInReport(val bytes: ULong, val sourceRemoved: Boolean)

/** Opaque ciphertext version from Rust. Size/time are display metadata, never a substitute for token. */
data class VaultTextVersion(val bytes: ULong, val modifiedMs: ULong?, val token: String)
data class VaultTextDocument(val text: String, val encoding: TextEncoding, val version: VaultTextVersion)
