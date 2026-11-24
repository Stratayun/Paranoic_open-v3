//! Token protection at rest. Tokens are serialized into a zeroizing buffer and
//! sealed (XChaCha20-Poly1305) with a key the host obtains from the keyring
//! session or platform keystore; the AEAD context binds provider + account, so
//! a blob cannot be swapped between accounts. Never write tokens unsealed, to
//! logs, crash reports, system backup or the journal.
use paranoic_crypto::SecretKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, ProviderKind, Result, TokenSet};

/// Host persistence for sealed blobs (app-private, excluded from backup).
pub trait TokenStore: Send + Sync {
    fn load(&self, provider: ProviderKind, account: &str) -> Result<Option<Vec<u8>>>;
    /// Replace atomically; on failure the previous blob must remain readable.
    fn save(&self, provider: ProviderKind, account: &str, sealed: &[u8]) -> Result<()>;
    fn remove(&self, provider: ProviderKind, account: &str) -> Result<()>;
}

#[derive(Serialize, Deserialize)]
struct Stored { v: u8, access: String, refresh: Option<String>, expires: Option<u64>, host: Option<String> }
impl Drop for Stored {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access.zeroize(); if let Some(r) = &mut self.refresh { r.zeroize(); }
    }
}

fn context(provider: ProviderKind, account: &str) -> Result<Vec<u8>> {
    if account.is_empty() || account.len() > 256 { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid cloud account id")); }
    let name: &[u8] = match provider { ProviderKind::GoogleDrive => b"gdrive", ProviderKind::OneDrive => b"onedrive", ProviderKind::PCloud => b"pcloud", ProviderKind::WebDav => b"webdav" };
    let mut out = b"paranoic/cloud-token/v1/".to_vec();
    out.extend_from_slice(name); out.push(0); out.extend_from_slice(&(account.len() as u32).to_le_bytes()); out.extend_from_slice(account.as_bytes());
    Ok(out)
}

pub fn seal_tokens(key: &SecretKey, provider: ProviderKind, account: &str, tokens: &TokenSet) -> Result<Vec<u8>> {
    let stored = Stored { v: 1, access: tokens.access.to_string(), refresh: tokens.refresh.as_ref().map(|r| r.to_string()),
        expires: tokens.expires_at_unix, host: tokens.api_host.clone() };
    let plain = Zeroizing::new(serde_json::to_vec(&stored).map_err(|_| CoreError::new(ErrorCode::Internal, "token encoding failed"))?);
    key.seal(&plain, &context(provider, account)?).map_err(|_| CoreError::new(ErrorCode::Io, "token sealing failed"))
}

pub fn open_tokens(key: &SecretKey, provider: ProviderKind, account: &str, sealed: &[u8]) -> Result<TokenSet> {
    let plain = key.open(sealed, &context(provider, account)?)
        .map_err(|_| CoreError::new(ErrorCode::Integrity, "stored cloud tokens are invalid or belong to another account"))?;
    let mut stored: Stored = serde_json::from_slice(&plain).map_err(|_| CoreError::new(ErrorCode::Integrity, "stored cloud tokens are malformed"))?;
    if stored.v != 1 { return Err(CoreError::new(ErrorCode::Unsupported, "unknown token record version")); }
    Ok(TokenSet { access: Zeroizing::new(std::mem::take(&mut stored.access)), refresh: stored.refresh.take().map(Zeroizing::new),
        expires_at_unix: stored.expires, api_host: stored.host.take() })
}
