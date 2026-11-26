//! Cryptomator v8 namespace primitives, independent of filesystem authority.
use aes_siv::{siv::Aes256Siv, aead::KeyInit, Tag};
use base64::{Engine, engine::general_purpose::URL_SAFE};
use data_encoding::BASE32;
use paranoic_crypto::{CryptoError, Result};
use sha1::{Digest, Sha1};
use zeroize::Zeroizing;
use rand_core::{OsRng, RngCore};

const MAX_NAME: usize = 255;
const MAX_ENCODED: usize = 364; // ceil((255 + 16) / 3) * 4

/// The empty ID denotes the root. Directory IDs are metadata, not paths.
/// No Debug/serde: IDs can correlate decrypted directory structure.
pub struct CryptomatorDirectoryId(Zeroizing<String>);

impl CryptomatorDirectoryId {
    pub fn root() -> Self { Self(Zeroizing::new(String::new())) }

    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 16];
        OsRng.try_fill_bytes(&mut bytes).map_err(|_| CryptoError::Entropy)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(Self(Zeroizing::new(uuid::Uuid::from_bytes(bytes).to_string())))
    }

    /// Read dir.c9r payload. This profile accepts canonical UUIDs and root only.
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() { return Ok(Self::root()); }
        if value.len() != 36 { return Err(CryptoError::InvalidInput); }
        let id = uuid::Uuid::parse_str(value).map_err(|_| CryptoError::InvalidInput)?;
        if id.to_string() != value { return Err(CryptoError::InvalidInput); }
        Ok(Self(Zeroizing::new(value.to_owned())))
    }

    /// Serialize only into the vault's directory-link object, never a log.
    pub fn as_bytes(&self) -> &[u8] { self.0.as_bytes() }
}

/// AES-SIV key in RustCrypto order: MAC half followed by encryption half.
/// Caller must authenticate the vault configuration before supplying keys.
pub struct CryptomatorNameKey(Zeroizing<[u8; 64]>);

impl CryptomatorNameKey {
    /// Input order is Cryptomator's encryption half followed by MAC half.
    pub fn from_unwrapped_master_key(bundle: Zeroizing<[u8; 64]>) -> Self {
        let mut siv_key = Zeroizing::new([0u8; 64]);
        siv_key[..32].copy_from_slice(&bundle[32..]);
        siv_key[32..].copy_from_slice(&bundle[..32]);
        Self(siv_key)
    }

    /// Full encrypted filename including .c9r. Does not shorten long names.
    /// The authenticated shorteningThreshold must be applied by the vault layer
    /// before publishing this component on a filesystem.
    pub fn encrypt_name(&self, parent: &CryptomatorDirectoryId, name: &str) -> Result<String> {
        // Cryptomator stores names in NFC; a decomposed (macOS) name would not match its entry.
        let name: String = unicode_normalization::UnicodeNormalization::nfc(name).collect();
        validate_name(&name)?;
        let mut payload = Zeroizing::new(name.as_bytes().to_vec());
        let tag = self.cipher()?.encrypt_in_place_detached(
            [parent.as_bytes()], &mut payload[..],
        ).map_err(|_| CryptoError::Authentication)?;
        let mut encoded = Vec::with_capacity(16 + payload.len());
        encoded.extend_from_slice(&tag);
        encoded.extend_from_slice(&payload);
        Ok(format!("{}.c9r", URL_SAFE.encode(encoded)))
    }

    /// Accepts the full .c9r name (including a resolved name.c9s mapping).
    /// Short .c9s wrapper names themselves are deliberately not accepted.
    pub fn decrypt_name(
        &self, parent: &CryptomatorDirectoryId, component: &str,
    ) -> Result<Zeroizing<String>> {
        if component.len() > MAX_ENCODED + 4 { return Err(CryptoError::LimitExceeded); }
        let encoded = component.strip_suffix(".c9r").ok_or(CryptoError::InvalidInput)?;
        let mut buffer = Zeroizing::new(URL_SAFE.decode(encoded).map_err(|_| CryptoError::InvalidInput)?);
        if !(17..=MAX_NAME + 16).contains(&buffer.len()) || URL_SAFE.encode(&buffer[..]) != encoded {
            return Err(CryptoError::InvalidInput);
        }
        let tag = Tag::clone_from_slice(&buffer[..16]);
        self.cipher()?.decrypt_in_place_detached(
            [parent.as_bytes()], &mut buffer[16..], &tag,
        ).map_err(|_| CryptoError::Authentication)?;
        let name = std::str::from_utf8(&buffer[16..]).map_err(|_| CryptoError::InvalidInput)?;
        validate_name(name)?;
        Ok(Zeroizing::new(name.to_owned()))
    }

    /// Relative ciphertext path d/XX/REMAINING_BASE32.
    /// SHA-1 is mandated by the format as a locator, not used as authentication.
    pub fn directory_path(&self, directory: &CryptomatorDirectoryId) -> Result<String> {
        let mut payload = Zeroizing::new(directory.as_bytes().to_vec());
        // No associated-data items here; one empty item would give a different SIV.
        let tag = self.cipher()?.encrypt_in_place_detached(
            std::iter::empty::<&[u8]>(), &mut payload[..],
        ).map_err(|_| CryptoError::Authentication)?;
        let mut hash = Sha1::new();
        hash.update(tag);
        hash.update(&payload[..]);
        let encoded = BASE32.encode(&hash.finalize());
        Ok(format!("d/{}/{}", &encoded[..2], &encoded[2..]))
    }

    fn cipher(&self) -> Result<Aes256Siv> {
        Aes256Siv::new_from_slice(&self.0[..]).map_err(|_| CryptoError::InvalidInput)
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME || name == "." || name == ".."
        || name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(CryptoError::InvalidInput);
    }
    Ok(())
}
