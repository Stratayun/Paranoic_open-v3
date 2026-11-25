//! Authenticated envelopes, bounded streaming and password-wrapped keys.
//! This is not a Vault implementation. Secrets are non-serializable.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, XChaCha20Poly1305, XNonce};
pub mod passphrase;
pub use passphrase::{contains_common, generate_passphrase, generate_password, has_repeats, has_sequence};
use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

mod session;
mod stream;
mod disk;
mod keyring;
mod lifecycle;
pub use lifecycle::{LifecycleEvent, SessionHandle, SessionManager};
pub use keyring::{KeyPage, KeyRecord, PasswordKeyring};
mod engine;
pub use engine::CryptoEngine;
pub use disk::{PrivateDirectory, ObjectId};
pub use session::KeySession;
pub use stream::{StreamLimits, StreamSink};

const MAX_PLAINTEXT: usize = 16 * 1024 * 1024;
const MAX_CONTEXT: usize = 4096;
const WRAPPED_KEY_SIZE: usize = 8 + 16 + 24 + 48;
const KEY_MAGIC: &[u8; 8] = b"PRNKEY01";
const DATA_MAGIC: &[u8; 8] = b"PRNDAT01";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("password attempts temporarily limited; retry after {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("key record not found")]
    NotFound,
    #[error("key store is already in use")]
    Busy,
    #[error("key store failure or unsupported schema")]
    KeyStore,
    #[error("key revision changed")]
    RevisionConflict,
    #[error("private storage permissions are insufficient")]
    Permissions,
    #[error("private storage operation unsupported")]
    Unsupported,
    #[error("destination already exists")]
    AlreadyExists,
    #[error("publication may have succeeded; inspect destination before retrying")]
    PublicationUncertain,
    #[error("stream I/O failed")]
    Io,
    #[error("stream limit exceeded")]
    LimitExceeded,
    #[error("stream operation cancelled")]
    Cancelled,
    #[error("temporary output cleanup could not be confirmed")]
    CleanupFailed,
    #[error("key session is locked or expired")]
    Locked,
    #[error("invalid cryptographic input")]
    InvalidInput,
    #[error("authentication failed")]
    Authentication,
    #[error("operating system entropy unavailable")]
    Entropy,
    #[error("key derivation failed")]
    Derivation,
}

pub type Result<T> = std::result::Result<T, CryptoError>;

// Keep provider paths and potentially sensitive OS error strings out of the
// public crypto error. Authentication/truncation checks map errors explicitly.
impl From<std::io::Error> for CryptoError {
    fn from(_: std::io::Error) -> Self { Self::Io }
}

/// Deliberately no Debug, Clone or serde. One owner controls key lifetime.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; 32]);

impl SecretKey {
    pub fn generate() -> Result<Self> {
        let mut key = Self([0; 32]);
        random(&mut key.0)?;
        Ok(key)
    }

    /// Encrypt a bounded object. Context must bind its domain and identity,
    /// e.g. vault UUID + object UUID + record type, using an unambiguous encoding.
    pub fn seal(&self, plaintext: &[u8], context: &[u8]) -> Result<Vec<u8>> {
        validate_context(context)?;
        if plaintext.len() > MAX_PLAINTEXT { return Err(CryptoError::InvalidInput); }
        let mut nonce = [0_u8; 24];
        random(&mut nonce)?;
        let associated = associated_data(DATA_MAGIC, context);
        let ciphertext = self.cipher()?.encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &associated })
            .map_err(|_| CryptoError::Authentication)?;
        let mut output = Vec::with_capacity(32 + ciphertext.len());
        output.extend_from_slice(DATA_MAGIC);
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn open(&self, envelope: &[u8], context: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        validate_context(context)?;
        if !(48..=MAX_PLAINTEXT + 48).contains(&envelope.len()) || &envelope[..8] != DATA_MAGIC {
            return Err(CryptoError::InvalidInput);
        }
        let associated = associated_data(DATA_MAGIC, context);
        self.cipher()?.decrypt(XNonce::from_slice(&envelope[8..32]), Payload { msg: &envelope[32..], aad: &associated })
            .map(Zeroizing::new).map_err(|_| CryptoError::Authentication)
    }

    /// Password wrapping does not change the random data key. Version 1 fixes
    /// Argon2id parameters, preventing attacker-controlled KDF resource costs.
    /// Call off the UI thread and rate-limit unlock attempts at the host boundary.
    pub fn wrap(&self, password: &[u8], context: &[u8]) -> Result<Vec<u8>> {
        validate_context(context)?;
        validate_password(password)?;
        let mut salt = [0_u8; 16];
        let mut nonce = [0_u8; 24];
        random(&mut salt)?;
        random(&mut nonce)?;
        let wrapping_key = derive(password, &salt)?;
        let mut associated = associated_data(KEY_MAGIC, context);
        associated.extend_from_slice(&salt);
        let ciphertext = wrapping_key.cipher()?.encrypt(XNonce::from_slice(&nonce), Payload { msg: &self.0, aad: &associated })
            .map_err(|_| CryptoError::Authentication)?;
        let mut output = Vec::with_capacity(WRAPPED_KEY_SIZE);
        output.extend_from_slice(KEY_MAGIC);
        output.extend_from_slice(&salt);
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn unwrap(envelope: &[u8], password: &[u8], context: &[u8]) -> Result<Self> {
        validate_context(context)?;
        validate_password(password)?;
        if envelope.len() != WRAPPED_KEY_SIZE || &envelope[..8] != KEY_MAGIC {
            return Err(CryptoError::InvalidInput);
        }
        let salt: &[u8; 16] = envelope[8..24].try_into().map_err(|_| CryptoError::InvalidInput)?;
        let wrapping_key = derive(password, salt)?;
        let mut associated = associated_data(KEY_MAGIC, context);
        associated.extend_from_slice(salt);
        let plaintext = Zeroizing::new(wrapping_key.cipher()?.decrypt(
            XNonce::from_slice(&envelope[24..48]), Payload { msg: &envelope[48..], aad: &associated },
        ).map_err(|_| CryptoError::Authentication)?);
        if plaintext.len() != 32 { return Err(CryptoError::Authentication); }
        let mut key = Self([0; 32]);
        key.0.copy_from_slice(&plaintext);
        Ok(key)
    }

    /// Per-operation copy for long streams; zeroized on drop like the original.
    pub(crate) fn duplicate(&self) -> Self { Self(self.0) }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.0).map_err(|_| CryptoError::InvalidInput)
    }
}

fn derive(password: &[u8], salt: &[u8; 16]) -> Result<SecretKey> {
    // 64 MiB, three passes, one lane; exactly 32 output bytes.
    let params = Params::new(65_536, 3, 1, Some(32)).map_err(|_| CryptoError::Derivation)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = SecretKey([0; 32]);
    argon.hash_password_into(password, salt, &mut key.0).map_err(|_| CryptoError::Derivation)?;
    Ok(key)
}

fn random(output: &mut [u8]) -> Result<()> {
    OsRng.try_fill_bytes(output).map_err(|_| CryptoError::Entropy)
}
/// Fill `output` from the operating system's secure random source.
pub fn random_bytes(output: &mut [u8]) -> Result<()> { random(output) }

fn validate_password(password: &[u8]) -> Result<()> {
    if password.is_empty() || password.len() > 4096 { Err(CryptoError::InvalidInput) } else { Ok(()) }
}

fn validate_context(context: &[u8]) -> Result<()> {
    if context.is_empty() || context.len() > MAX_CONTEXT { Err(CryptoError::InvalidInput) } else { Ok(()) }
}

fn associated_data(magic: &[u8; 8], context: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + context.len());
    data.extend_from_slice(magic);
    data.extend_from_slice(&(context.len() as u64).to_le_bytes());
    data.extend_from_slice(context);
    data
}

