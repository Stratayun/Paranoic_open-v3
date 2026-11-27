//! Password unlock and authenticated namespace configuration for vault formats 7 and 8.
use std::io::Read;
use aes_kw::KekAes256;
use base64::{Engine, engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD}};
use hmac::{Hmac, Mac};
use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha384, Sha512};
use serde::{Deserialize, Serialize};
use rand_core::{OsRng, RngCore};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;
use paranoic_crypto::{CryptoError, Result, StreamSink};
use crate::{CryptomatorNameKey, CryptomatorGcmKey, CryptomatorDirectoryId,
    CryptomatorContentLimits, CryptomatorLengthPolicy, CryptomatorContentOutcome};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyFile {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    version_mac: Option<String>,
    scrypt_salt: String,
    scrypt_cost_param: u64,
    scrypt_block_size: u32,
    primary_master_key: String,
    hmac_master_key: String,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
    typ: Option<String>,
    crit: Option<Vec<String>>,
    b64: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claims {
    format: u32,
    cipher_combo: String,
    shortening_threshold: usize,
    jti: String,
}

/// Single-owner unlocked vault. Drop/lock wipes owned keys; host must serialize
/// access and deliver lifecycle cancellation to ongoing content operations.
/// This object grants no filesystem or Private Vault authority.
pub struct CryptomatorVault {
    master: Zeroizing<[u8; 64]>,
    cost: u64,
    block_size: u32,
    names: CryptomatorNameKey,
    content: Content,
    id: uuid::Uuid,
    threshold: usize,
    format: u32,
    /// "version" written into masterkey.cryptomator (999 for format 8, 7 for 7).
    key_version: u32,
}

enum Content { Gcm(CryptomatorGcmKey), CtrMac(crate::CryptomatorCtrMacKey) }

/// Content cipher of an unlocked vault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptomatorCipherCombo { SivGcm, SivCtrMac }

/// Ciphertext namespace output. A shortened name requires a directory wrapper
/// containing name.c9s and contents.c9r (file) or dir.c9r (directory link).
pub struct CryptomatorEntryName {
    component: String,
    mapping: Option<String>,
}

/// Prepared ciphertext objects, not a successful storage transaction.
/// Write key_file as masterkey.cryptomator, config as vault.cryptomator and
/// root_backup as root_path/dirid.c9r. Expose only after durable publication.
pub struct CryptomatorBootstrap {
    vault: CryptomatorVault,
    key_file: Vec<u8>,
    config: String,
    root_path: String,
    root_backup: Vec<u8>,
}

impl CryptomatorBootstrap {
    pub fn vault(&self) -> &CryptomatorVault { &self.vault }
    pub fn masterkey_file(&self) -> &[u8] { &self.key_file }
    pub fn config_file(&self) -> &str { &self.config }
    pub fn root_data_path(&self) -> &str { &self.root_path }
    pub fn root_directory_backup(&self) -> &[u8] { &self.root_backup }
    /// Queue the complete new vault as one create-new directory transaction.
    /// Persist job_id before calling; after an uncertain acknowledgement use the
    /// same ID and exact prepared bootstrap, never generate replacement keys.
    /// Only wrapped keys and ciphertext enter the durable journal. This method
    /// does not unlock a reader or imply that the queued job has succeeded.
    pub fn submit(&self, core: &paranoic_core::Core, target: paranoic_core::Location,
        job_id: paranoic_core::JobId) -> paranoic_core::Result<paranoic_core::JobRecord> {
        use paranoic_core::{Operation, PreparedTreeFile, ResourcePath};
        let files = vec![
            PreparedTreeFile { path: ResourcePath::parse("masterkey.cryptomator")?, bytes: self.key_file.clone() },
            PreparedTreeFile { path: ResourcePath::parse("vault.cryptomator")?, bytes: self.config.as_bytes().to_vec() },
            PreparedTreeFile { path: ResourcePath::parse(format!("{}/dirid.c9r", self.root_path))?, bytes: self.root_backup.clone() },
        ];
        core.submit_with_id(job_id, Operation::PublishTree { target, files })
    }

    pub fn into_vault(self) -> CryptomatorVault { self.vault }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WrappedKeyFile {
    version: u32,
    version_mac: String,
    scrypt_salt: String,
    scrypt_cost_param: u64,
    scrypt_block_size: u32,
    primary_master_key: String,
    hmac_master_key: String,
}

// Private bounded sink used ONLY for ciphertext directory backups, never plaintext.
struct CiphertextSink {
    bytes: Vec<u8>,
    result: std::rc::Rc<std::cell::RefCell<Option<Vec<u8>>>>,
}
impl std::io::Write for CiphertextSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > 172 {
            return Err(std::io::Error::other("directory backup exceeds limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for CiphertextSink {
    fn commit(self: Box<Self>) -> Result<()> {
        *self.result.borrow_mut() = Some(self.bytes);
        Ok(())
    }
    fn abort(self: Box<Self>) -> Result<()> { Ok(()) }
}

impl CryptomatorEntryName {
    pub fn component(&self) -> &str { &self.component }
    pub fn name_mapping(&self) -> Option<&str> { self.mapping.as_deref() }
}

impl CryptomatorVault {
    /// Prepare a new vault. This does not publish a directory or mutate storage.
    /// Caller must create all bootstrap objects together before exposing a vault.
    pub fn prepare_create<F: FnMut() -> Result<()>>(
        password: Zeroizing<String>, mut checkpoint: F,
    ) -> Result<CryptomatorBootstrap> {
        checkpoint()?;
        if password.is_empty() || password.len() > 4096 { return Err(CryptoError::InvalidInput); }
        let mut master = Zeroizing::new([0u8; 64]);
        OsRng.try_fill_bytes(&mut master[..]).map_err(|_| CryptoError::Entropy)?;
        let mut name_bundle = Zeroizing::new([0u8; 64]);
        name_bundle.copy_from_slice(&master[..]);
        let mut encryption = Zeroizing::new([0u8; 32]);
        encryption.copy_from_slice(&master[..32]);
        let mut id_bytes = [0u8; 16];
        OsRng.try_fill_bytes(&mut id_bytes).map_err(|_| CryptoError::Entropy)?;
        id_bytes[6] = (id_bytes[6] & 15) | 64;
        id_bytes[8] = (id_bytes[8] & 63) | 128;
        let vault = Self {
            // Audit 26.11: 2^16 (64 MiB), twice the official default and within this module's limits.
            master, cost: 65536, block_size: 8,
            names: CryptomatorNameKey::from_unwrapped_master_key(name_bundle),
            content: Content::Gcm(CryptomatorGcmKey::from_unwrapped_encryption_key(encryption)),
            id: uuid::Uuid::from_bytes(id_bytes), threshold: 220, format: 8, key_version: 999,
        };
        let key_file = vault.prepare_password_change(password, &mut checkpoint)?;
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"kid\":\"masterkeyfile:masterkey.cryptomator\",\"typ\":\"JWT\"}");
        let claims = serde_json::json!({"format":8,"cipherCombo":"SIV_GCM",
            "shorteningThreshold":vault.threshold,"jti":vault.id.to_string()});
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| CryptoError::InvalidInput)?);
        let signed = format!("{header}.{payload}");
        let signature = sign(&vault.master[..], signed.as_bytes())?;
        let config = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature));
        let root_path = vault.directory_path(&CryptomatorDirectoryId::root())?;
        let root_backup = vault.directory_backup(&CryptomatorDirectoryId::root(), &mut checkpoint)?;
        checkpoint()?;
        Ok(CryptomatorBootstrap { vault, key_file, config, root_path, root_backup })
    }

    /// Return a replacement wrapped-key document with fresh salt. The host must
    /// retain the old file until durable replacement is confirmed and revoke
    /// existing sessions separately. Old backups still accept the old password.
    pub fn prepare_password_change<F: FnMut() -> Result<()>>(
        &self, password: Zeroizing<String>, mut checkpoint: F,
    ) -> Result<Vec<u8>> {
        checkpoint()?;
        if password.is_empty() || password.len() > 4096 { return Err(CryptoError::InvalidInput); }
        let mut salt = [0u8; 16];
        OsRng.try_fill_bytes(&mut salt).map_err(|_| CryptoError::Entropy)?;
        // Never weaken imported parameters; new wrapping has at least the
        // standard creation work factor, while retaining its admitted r.
        let cost = self.cost.max(65536);
        let kek = derive_password(&password, &salt, cost, self.block_size, &mut checkpoint)?;
        checkpoint()?;
        let wrapper = KekAes256::try_from(&kek[..]).map_err(|_| CryptoError::InvalidInput)?;
        let mut enc = Zeroizing::new([0u8; 40]);
        let mut mac = Zeroizing::new([0u8; 40]);
        wrapper.wrap(&self.master[..32], &mut enc[..]).map_err(|_| CryptoError::Authentication)?;
        wrapper.wrap(&self.master[32..], &mut mac[..]).map_err(|_| CryptoError::Authentication)?;
        let file = WrappedKeyFile {
            version: self.key_version, version_mac: STANDARD.encode(sign(&self.master[32..], &self.key_version.to_be_bytes())?),
            scrypt_salt: STANDARD.encode(salt), scrypt_cost_param: cost, scrypt_block_size: self.block_size,
            primary_master_key: STANDARD.encode(&enc[..]), hmac_master_key: STANDARD.encode(&mac[..]),
        };
        checkpoint()?;
        serde_json::to_vec(&file).map_err(|_| CryptoError::InvalidInput)
    }

    /// Encrypted dirid.c9r recovery object. Publish it within the directory's
    /// data path before exposing the parent directory link.
    pub fn directory_backup<F: FnMut() -> Result<()>>(
        &self, id: &CryptomatorDirectoryId, mut checkpoint: F,
    ) -> Result<Vec<u8>> {
        let result = std::rc::Rc::new(std::cell::RefCell::new(None));
        let sink = CiphertextSink { bytes: Vec::new(), result: result.clone() };
        self.encrypt_content(id.as_bytes(), Box::new(sink),
            CryptomatorContentLimits { max_plaintext_bytes: 36 }, |_| checkpoint())?;
        let value = result.borrow_mut().take().ok_or(CryptoError::Io)?;
        Ok(value)
    }

    /// Format 8: local masterkeyfile credentials, cipherCombo SIV_GCM or
    /// SIV_CTRMAC. No network resolution of kid; Hub/pepper credentials rejected.
    pub fn unlock<F: FnMut() -> Result<()>>(
        masterkey_json: &[u8], token: &str, password: Zeroizing<String>,
        mut checkpoint: F,
    ) -> Result<Self> {
        checkpoint()?;
        if masterkey_json.len() > 16 * 1024 || token.len() > 16 * 1024 || password.len() > 4096 {
            return Err(CryptoError::LimitExceeded);
        }
        let mut parts = token.split('.');
        let h = parts.next().ok_or(CryptoError::InvalidInput)?;
        let p = parts.next().ok_or(CryptoError::InvalidInput)?;
        let s = parts.next().ok_or(CryptoError::InvalidInput)?;
        if parts.next().is_some() || h.len() > 2048 || p.len() > 8192 || s.len() > 86 {
            return Err(CryptoError::InvalidInput);
        }
        let header: Header = json(&decode_url(h)?)?;
        if header.kid != "masterkeyfile:masterkey.cryptomator"
            || !matches!(header.alg.as_str(), "HS256" | "HS384" | "HS512")
            || header.typ.as_deref().is_some_and(|v| v != "JWT")
            || header.crit.is_some() || header.b64 == Some(false)
        { return Err(CryptoError::Unsupported); }
        let signature = decode_url(s)?;
        let (bundle, n, r, key_version) = unwrap_key_file(masterkey_json, &password, &mut checkpoint)?;
        let signed = &token[..h.len() + 1 + p.len()];
        verify(&header.alg, &bundle[..], signed.as_bytes(), &signature)?;
        // Only now interpret claims as trusted configuration.
        let claims: Claims = json(&decode_url(p)?)?;
        let combo = match (claims.format, claims.cipher_combo.as_str()) {
            (8, "SIV_GCM") => CryptomatorCipherCombo::SivGcm,
            (8, "SIV_CTRMAC") => CryptomatorCipherCombo::SivCtrMac,
            _ => return Err(CryptoError::Unsupported),
        };
        if !(36..=255).contains(&claims.shortening_threshold) {
            return Err(CryptoError::Unsupported);
        }
        let id = uuid::Uuid::parse_str(&claims.jti).map_err(|_| CryptoError::InvalidInput)?;
        checkpoint()?;
        Ok(Self::assemble(bundle, n, r, combo, id, claims.shortening_threshold, 8, key_version.unwrap_or(999)))
    }

    /// Format 7 (no vault.cryptomator): masterkey "version" must be 7 and its
    /// versionMac must verify; SIV_CTRMAC, fixed shortening threshold 220.
    /// No vault UUID exists in this format: a stable ID is derived from the
    /// MAC key (HMAC, not reversible) for session binding only.
    pub fn unlock_legacy<F: FnMut() -> Result<()>>(
        masterkey_json: &[u8], password: Zeroizing<String>, mut checkpoint: F,
    ) -> Result<Self> {
        checkpoint()?;
        if masterkey_json.len() > 16 * 1024 || password.len() > 4096 { return Err(CryptoError::LimitExceeded); }
        let (bundle, n, r, key_version) = unwrap_key_file(masterkey_json, &password, &mut checkpoint)?;
        if key_version != Some(7) { return Err(CryptoError::Unsupported); }
        let derived = sign(&bundle[32..], b"paranoic/cryptomator-v7-vault-id")?;
        let mut id_bytes = [0u8; 16];
        id_bytes.copy_from_slice(&derived[..16]);
        let id = uuid::Builder::from_random_bytes(id_bytes).into_uuid();
        checkpoint()?;
        Ok(Self::assemble(bundle, n, r, CryptomatorCipherCombo::SivCtrMac, id, 220, 7, 7))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(bundle: Zeroizing<[u8; 64]>, cost: u64, block_size: u32, combo: CryptomatorCipherCombo,
        id: uuid::Uuid, threshold: usize, format: u32, key_version: u32) -> Self {
        let mut name_bundle = Zeroizing::new([0u8; 64]);
        name_bundle.copy_from_slice(&bundle[..]);
        let content = match combo {
            CryptomatorCipherCombo::SivGcm => {
                let mut encryption = Zeroizing::new([0u8; 32]);
                encryption.copy_from_slice(&bundle[..32]);
                Content::Gcm(CryptomatorGcmKey::from_unwrapped_encryption_key(encryption))
            }
            CryptomatorCipherCombo::SivCtrMac => Content::CtrMac(crate::CryptomatorCtrMacKey::from_unwrapped_master_key(&bundle)),
        };
        Self { master: bundle, cost, block_size, names: CryptomatorNameKey::from_unwrapped_master_key(name_bundle),
            content, id, threshold, format, key_version }
    }

    pub fn format(&self) -> u32 { self.format }
    pub fn cipher_combo(&self) -> CryptomatorCipherCombo {
        match self.content { Content::Gcm(_) => CryptomatorCipherCombo::SivGcm, Content::CtrMac(_) => CryptomatorCipherCombo::SivCtrMac }
    }

    pub fn id(&self) -> uuid::Uuid { self.id }
    pub fn lock(self) { drop(self); }
    pub fn directory_path(&self, id: &CryptomatorDirectoryId) -> Result<String> {
        self.names.directory_path(id)
    }
    pub fn encrypt_name(&self, parent: &CryptomatorDirectoryId, name: &str) -> Result<CryptomatorEntryName> {
        let full = self.names.encrypt_name(parent, name)?;
        if full.len() <= self.threshold {
            Ok(CryptomatorEntryName { component: full, mapping: None })
        } else {
            Ok(CryptomatorEntryName { component: shortened(&full), mapping: Some(full) })
        }
    }
    pub fn decrypt_name(&self, parent: &CryptomatorDirectoryId, component: &str, mapping: Option<&str>)
        -> Result<Zeroizing<String>>
    {
        if component.len() > self.threshold { return Err(CryptoError::InvalidInput); }
        if component.ends_with(".c9s") {
            let full = mapping.ok_or(CryptoError::InvalidInput)?;
            if full.len() <= self.threshold || full.len() > 368 || shortened(full) != component {
                return Err(CryptoError::Authentication);
            }
            self.names.decrypt_name(parent, full)
        } else {
            if mapping.is_some() { return Err(CryptoError::InvalidInput); }
            self.names.decrypt_name(parent, component)
        }
    }
    pub fn encrypt_content<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, input: R, output: Box<dyn StreamSink>, limits: CryptomatorContentLimits, progress: F,
    ) -> Result<u64> {
        match &self.content {
            Content::Gcm(key) => key.encrypt(input, output, limits, progress),
            Content::CtrMac(key) => key.encrypt(input, output, limits, progress),
        }
    }
    pub(crate) fn open_chunks(&self, header: &[u8]) -> Result<crate::cryptomator_random::Chunks> {
        Ok(match &self.content {
            Content::Gcm(key) => crate::cryptomator_random::Chunks::Gcm(Box::new(key.open_header(header)?)),
            Content::CtrMac(key) => crate::cryptomator_random::Chunks::CtrMac(key.open_header(header)?),
        })
    }
    pub fn decrypt_content<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, input: R, output: Box<dyn StreamSink>, limits: CryptomatorContentLimits,
        length: CryptomatorLengthPolicy, progress: F,
    ) -> Result<CryptomatorContentOutcome> {
        match &self.content {
            Content::Gcm(key) => key.decrypt(input, output, limits, length, progress),
            Content::CtrMac(key) => key.decrypt(input, output, limits, length, progress),
        }
    }
}

/// Unwrap both key halves and authenticate the optional "version" field.
#[allow(clippy::type_complexity)]
fn unwrap_key_file<F: FnMut() -> Result<()>>(masterkey_json: &[u8], password: &str, checkpoint: &mut F)
    -> Result<(Zeroizing<[u8; 64]>, u64, u32, Option<u32>)> {
    let file: KeyFile = json(masterkey_json)?;
    let salt = decode_standard(&file.scrypt_salt, 64)?;
    let enc = decode_standard(&file.primary_master_key, 40)?;
    let mac = decode_standard(&file.hmac_master_key, 40)?;
    if !(8..=64).contains(&salt.len()) || enc.len() != 40 || mac.len() != 40 {
        return Err(CryptoError::InvalidInput);
    }
    let n = file.scrypt_cost_param;
    let r = file.scrypt_block_size;
    checkpoint()?;
    let kek = derive_password(password, &salt, n, r, checkpoint)?;
    checkpoint()?;
    let wrapper = KekAes256::try_from(&kek[..]).map_err(|_| CryptoError::InvalidInput)?;
    let mut bundle = Zeroizing::new([0u8; 64]);
    wrapper.unwrap(&enc, &mut bundle[..32]).map_err(|_| CryptoError::Authentication)?;
    wrapper.unwrap(&mac, &mut bundle[32..]).map_err(|_| CryptoError::Authentication)?;
    match (file.version, file.version_mac.as_deref()) {
        (Some(version), Some(text)) => {
            let expected = decode_standard(text, 32)?;
            let mut check = <Hmac<Sha256> as Mac>::new_from_slice(&bundle[32..]).map_err(|_| CryptoError::InvalidInput)?;
            check.update(&version.to_be_bytes());
            check.verify_slice(&expected).map_err(|_| CryptoError::Authentication)?;
        }
        (None, None) => {}
        _ => return Err(CryptoError::InvalidInput),
    }
    Ok((bundle, n, r, file.version))
}

fn shortened(full: &str) -> String { format!("{}.c9s", URL_SAFE.encode(Sha1::digest(full.as_bytes()))) }
fn sign(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| CryptoError::InvalidInput)?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| CryptoError::InvalidInput)
}
fn decode_url(text: &str) -> Result<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| CryptoError::InvalidInput)?;
    if URL_SAFE_NO_PAD.encode(&bytes) != text { return Err(CryptoError::InvalidInput); }
    Ok(bytes)
}
fn decode_standard(text: &str, max: usize) -> Result<Vec<u8>> {
    if text.len() > max.div_ceil(3) * 4 { return Err(CryptoError::LimitExceeded); }
    let bytes = STANDARD.decode(text).map_err(|_| CryptoError::InvalidInput)?;
    if bytes.len() > max { return Err(CryptoError::LimitExceeded); }
    Ok(bytes)
}
fn verify(alg: &str, key: &[u8], data: &[u8], signature: &[u8]) -> Result<()> {
    macro_rules! check {
        ($hash:ty) => {{
            let mut mac = <Hmac<$hash> as Mac>::new_from_slice(key).map_err(|_| CryptoError::InvalidInput)?;
            mac.update(data);
            mac.verify_slice(signature).map_err(|_| CryptoError::Authentication)
        }};
    }
    match alg { "HS256" => check!(Sha256), "HS384" => check!(Sha384), "HS512" => check!(Sha512),
        _ => Err(CryptoError::Unsupported) }
}

fn derive_password(password: &str, salt: &[u8], n: u64, r: u32, checkpoint: &mut dyn FnMut() -> Result<()>)
    -> Result<Zeroizing<[u8; 32]>> {
    if password.len() > 4096 { return Err(CryptoError::LimitExceeded); }
        if !n.is_power_of_two() || n < 2 || r == 0 || r > 32 {
            return Err(CryptoError::InvalidInput);
        }
        // Bound memory plus scratch, and total work before allocating KDF state.
        let work = n.checked_mul(u64::from(r)).ok_or(CryptoError::LimitExceeded)?;
        let memory = work.checked_mul(128).and_then(|v| v.checked_add(u64::from(r) * 256 + 65536))
            .ok_or(CryptoError::LimitExceeded)?;
        if work > 1_048_576 || memory > 128 * 1024 * 1024 { return Err(CryptoError::LimitExceeded); }
        // The process-wide KDF budget (A28), shared with 1.8, Safe and age.
        let _permit = paranoic_crypto::kdf_gate::admit(memory.div_ceil(1024), checkpoint)?;
        let mut normalized = Zeroizing::new(String::new());
        for ch in password.nfc() {
            if normalized.len() + ch.len_utf8() > 4096 { return Err(CryptoError::LimitExceeded); }
            normalized.push(ch);
        }
        let params = scrypt::Params::new(n.ilog2() as u8, r, 1, 32)
            .map_err(|_| CryptoError::InvalidInput)?;
        let mut kek = Zeroizing::new([0u8; 32]);
        scrypt::scrypt(normalized.as_bytes(), salt, &params, &mut kek[..])
            .map_err(|_| CryptoError::Derivation)?;
    Ok(kek)
}
