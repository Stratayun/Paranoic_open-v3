//! Cryptomator vaults in the cloud (roadmap 3.9). The vault engine in
//! paranoic-formats works on any Core `Storage`: register a `CloudStorage`
//! grant in Core and open/create the vault at a path in it exactly as on local
//! storage (`CryptomatorManager::open_reader(Arc<CloudStorage>, root, ...)`,
//! `prepare_*`/`submit_*`). Encryption, names and keys stay on the device; the
//! provider stores only ciphertext. Writes reach the cloud only through Core
//! jobs (PublishTree, CopyExpected/Bundle, Move, Delete*), each create-new.
//!
//! Writable vaults: Google Drive, OneDrive and WebDAV (own server, Nextcloud)
//! — providers with atomic relocate without replacement — including renames
//! that change name shortening (RenameMapped / ReshapeDirectory with recovery,
//! see mapped.rs). pCloud: read-only vaults until its no-overwrite rename
//! semantics are verified on a live account.
use paranoic_core::Storage;
use crate::{CloudStorage, CoreError, ErrorCode, Result};

/// Fail early (before a KDF or vault creation) if this cloud grant cannot
/// host a writable Cryptomator vault. Read-only access works on every provider.
pub fn check_vault_support(storage: &CloudStorage, writable: bool) -> Result<()> {
    let capabilities = storage.capabilities();
    if !capabilities.read { return Err(CoreError::new(ErrorCode::PermissionDenied, "cloud grant is not readable")); }
    if writable && !(capabilities.write && capabilities.atomic_move_no_replace && capabilities.durable_directory_sync
        && capabilities.delete && capabilities.conditional_move_source) {
        return Err(CoreError::new(ErrorCode::Unsupported, "this cloud provider cannot host a writable vault safely"));
    }
    Ok(())
}
