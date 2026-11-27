//! Paranoic encrypted folder format v1 (EXPERIMENTAL; spec:
//! docs/format/paranoic-v1.md). This module is the cryptographic core:
//! vault header and key slots, object naming, entry headers and the chunked
//! content stream with optional Padmé padding. Storage operations through Core
//! and the cloud-dependent parts (spec §5 index, §8 conditional replace) are
//! separate. The §9 version manifest is in `paranoic_manifest` (header flag bit 1).
use std::io::{Read, Write};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use paranoic_crypto::{CryptoError, Result, StreamSink};
use sha2::Sha256;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

pub const VAULT_FILE: &str = "paranoic.vault";
const MAGIC: &[u8; 8] = b"PRNVLT01";
const VERSION: u16 = 1;
const SLOT_LEN: usize = 74;
const FIXED_LEN: usize = 8 + 2 + 16 + 2 + 4 + 4 + 4 + 32 + 1;
const ENTRY_MAGIC: &[u8; 4] = b"PRNE";
const PAYLOAD_LEN: usize = 328;
pub const ENTRY_HEADER_LEN: usize = 4 + 1 + 1 + 24 + PAYLOAD_LEN + 16;
const CHUNK: usize = 64 * 1024;
const NAME_MAX: usize = 255;

/// Source of randomness. `OsRandom` in production; deterministic sources only
/// for test vectors.
pub trait RandomSource { fn fill(&mut self, out: &mut [u8]) -> Result<()>; }
pub struct OsRandom;
impl RandomSource for OsRandom {
    fn fill(&mut self, out: &mut [u8]) -> Result<()> {
        use rand_core::{OsRng, RngCore};
        OsRng.try_fill_bytes(out).map_err(|_| CryptoError::Entropy)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KdfParams { pub memory_kib: u32, pub iterations: u32, pub lanes: u32 }
impl KdfParams {
    pub const DESKTOP: Self = Self { memory_kib: 256 * 1024, iterations: 3, lanes: 1 };
    pub const PHONE: Self = Self { memory_kib: 128 * 1024, iterations: 3, lanes: 1 };
    fn validate(&self) -> Result<()> {
        if !(65_536..=1_048_576).contains(&self.memory_kib) || !(1..=10).contains(&self.iterations) || !(1..=4).contains(&self.lanes) {
            return Err(CryptoError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotKind { Password = 1, Recovery = 2 }

/// Unlocked vault keys. Master key kept only for password/slot changes.
pub struct VaultKeys {
    pub vault_id: [u8; 16],
    pub padding: bool,
    /// Header flag bit 1: the version manifest is mandatory (spec §9).
    pub manifest_required: bool,
    master: Zeroizing<[u8; 32]>,
    manifest: Zeroizing<[u8; 32]>,
    file_header: Zeroizing<[u8; 32]>,
    name: Zeroizing<[u8; 32]>,
    folder: Zeroizing<[u8; 32]>,
}

struct Header { vault_id: [u8; 16], flags: u16, kdf: KdfParams, salt: [u8; 32], slots: Vec<(u8, [u8; 24], [u8; 48])> }

/// Create a vault header with a password slot. Returns header bytes and keys.
pub fn create_vault(password: &[u8], kdf: KdfParams, padding: bool, rng: &mut dyn RandomSource,
    checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<(Vec<u8>, VaultKeys)> {
    kdf.validate()?; validate_password(password)?;
    let mut vault_id = [0u8; 16]; rng.fill(&mut vault_id)?;
    let mut salt = [0u8; 32]; rng.fill(&mut salt)?;
    let mut master = Zeroizing::new([0u8; 32]); rng.fill(master.as_mut())?;
    let mut header = Header { vault_id, flags: u16::from(padding), kdf, salt, slots: Vec::new() };
    let kek = password_kek(password, &header, checkpoint)?;
    header.slots.push(seal_slot(&kek, &master, &vault_id, SlotKind::Password, rng)?);
    let keys = derive_keys(&master, &header)?;
    Ok((encode(&header, &master)?, keys))
}

/// Parse a header, unwrap the master key with the password and verify the header MAC.
pub fn unlock(bytes: &[u8], password: &[u8], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<VaultKeys> {
    validate_password(password)?;
    let header = decode(bytes)?;
    let kek = password_kek(password, &header, checkpoint)?;
    open_with(&header, bytes, &kek, SlotKind::Password)
}

/// Unlock with the 32-byte recovery key.
pub fn unlock_recovery(bytes: &[u8], recovery: &[u8; 32]) -> Result<VaultKeys> {
    let header = decode(bytes)?;
    let kek = recovery_kek(recovery, &header.vault_id)?;
    open_with(&header, bytes, &kek, SlotKind::Recovery)
}

/// New password slot; contents untouched. Does not revoke old header copies
/// (spec §3): master-key rotation is a separate operation.
pub fn change_password(bytes: &[u8], keys: &VaultKeys, new_password: &[u8], rng: &mut dyn RandomSource,
    checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Vec<u8>> {
    validate_password(new_password)?;
    let mut header = decode(bytes)?;
    verify_mac(&header, bytes, &keys.master)?;
    // Audit 26.11: a fresh salt, so an old header copy and the new one cannot be
    // attacked in one dictionary pass. Only the password slot uses it.
    rng.fill(&mut header.salt)?;
    let kek = password_kek(new_password, &header, checkpoint)?;
    header.slots.retain(|s| s.0 != SlotKind::Password as u8);
    header.slots.insert(0, seal_slot(&kek, &keys.master, &header.vault_id, SlotKind::Password, rng)?);
    encode(&header, &keys.master)
}

/// Same header with flag bit 1 (version manifest required, spec §9), re-MACed.
/// Write it only after the manifest itself has been published.
pub fn require_manifest(bytes: &[u8], keys: &VaultKeys) -> Result<Vec<u8>> {
    let mut header = decode(bytes)?;
    verify_mac(&header, bytes, &keys.master)?;
    header.flags |= 2;
    encode(&header, &keys.master)
}

/// Add (or replace) the recovery slot; returns the new header and the recovery key to show once.
pub fn set_recovery(bytes: &[u8], keys: &VaultKeys, rng: &mut dyn RandomSource) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>)> {
    let mut header = decode(bytes)?;
    verify_mac(&header, bytes, &keys.master)?;
    let mut recovery = Zeroizing::new([0u8; 32]); rng.fill(recovery.as_mut())?;
    let kek = recovery_kek(&recovery, &header.vault_id)?;
    header.slots.retain(|s| s.0 != SlotKind::Recovery as u8);
    header.slots.push(seal_slot(&kek, &keys.master, &header.vault_id, SlotKind::Recovery, rng)?);
    Ok((encode(&header, &keys.master)?, recovery))
}

/// Recovery key for display: lowercase base32 in groups of 4.
pub fn recovery_text(key: &[u8; 32]) -> Zeroizing<String> {
    let raw = Zeroizing::new(base32(key));
    let mut out = String::with_capacity(raw.len() + raw.len() / 4);
    for (i, c) in raw.chars().enumerate() { if i > 0 && i % 4 == 0 { out.push('-'); } out.push(c); }
    Zeroizing::new(out)
}

/// Groups of 4 characters in `recovery_text` (32 bytes = 52 base32 chars).
pub const RECOVERY_GROUPS: usize = 13;

/// Plan 5.4: before the recovery key window may close, the user re-types
/// `count` (2 or 3) randomly chosen groups. Distinct 0-based indices, ascending.
pub fn recovery_challenge(count: usize, rng: &mut dyn RandomSource) -> Result<Vec<usize>> {
    if !(2..=3).contains(&count) { return Err(CryptoError::InvalidInput); }
    let mut picked: Vec<usize> = Vec::with_capacity(count);
    while picked.len() < count {
        let mut byte = [0u8; 1];
        rng.fill(&mut byte)?;
        // 13 × 19 = 247: reject the top to keep the choice uniform.
        if byte[0] >= 247 { continue; }
        let index = usize::from(byte[0]) % RECOVERY_GROUPS;
        if !picked.contains(&index) { picked.push(index); }
    }
    picked.sort_unstable();
    Ok(picked)
}

/// Whether `answers` are the groups at `indices` of the shown key `text`.
/// Case, spaces and dashes in the answers are ignored; compared in
/// constant time per group; a wrong count or index is simply false.
pub fn recovery_check(text: &str, indices: &[usize], answers: &[&str]) -> bool {
    let groups: Vec<Zeroizing<String>> = text.split('-').map(|g| Zeroizing::new(g.to_ascii_uppercase())).collect();
    if groups.len() != RECOVERY_GROUPS || indices.len() != answers.len() || !(2..=3).contains(&indices.len()) { return false; }
    let mut ok = 1u8;
    for (&index, answer) in indices.iter().zip(answers) {
        let Some(expected) = groups.get(index) else { return false };
        let typed = Zeroizing::new(answer.chars().filter(|c| !matches!(c, '-' | ' ' | '\t')).map(|c| c.to_ascii_uppercase()).collect::<String>());
        let same = typed.len() == expected.len()
            && typed.bytes().zip(expected.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        ok &= u8::from(same);
    }
    ok == 1
}

/// Parse a recovery key typed by the user: `recovery_text` form, case,
/// dashes and spaces ignored. Exactly 32 bytes or `InvalidInput`.
pub fn parse_recovery_text(text: &str) -> Result<Zeroizing<[u8; 32]>> {
    if text.len() > 256 { return Err(CryptoError::InvalidInput); }
    let raw: Zeroizing<String> = Zeroizing::new(text.chars().filter(|c| !matches!(c, '-' | ' ' | '\t' | '\n' | '\r'))
        .map(|c| c.to_ascii_uppercase()).collect());
    let bytes = Zeroizing::new(data_encoding::BASE32_NOPAD.decode(raw.as_bytes()).map_err(|_| CryptoError::InvalidInput)?);
    let mut key = Zeroizing::new([0u8; 32]);
    if bytes.len() != 32 { return Err(CryptoError::InvalidInput); }
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind { File = 1, Folder = 2 }

/// Where an entry goes on rename/move: folder, name and the new mtime.
pub struct NewPlace<'a> { pub folder: &'a [u8; 16], pub name: &'a str, pub modified_ms: u64 }

/// Plaintext file being stored: its name in the folder, mtime and exact length.
pub struct NewFile<'a> { pub name: &'a str, pub modified_ms: u64, pub size: u64 }

/// Decrypted entry header.
#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    File { name: String, modified_ms: u64, size: u64 },
    Folder { name: String, modified_ms: u64, folder_id: [u8; 16] },
}

impl VaultKeys {
    pub(crate) fn manifest_key(&self) -> &[u8; 32] { &self.manifest }
    /// Storage folder of a plaintext folder ("d/aa/<30>"). Root: `folder_id == vault_id`.
    pub fn folder_path(&self, folder_id: &[u8; 16]) -> Result<String> {
        let digest = base32(&hmac(&self.folder, &[folder_id])?);
        Ok(format!("d/{}/{}", &digest[..2], &digest[2..32]))
    }
    /// Object name of an entry in `folder_id` (26 base32 chars + ".f"/".d").
    pub fn entry_name(&self, folder_id: &[u8; 16], name: &str, kind: EntryKind) -> Result<String> {
        let name = normalized(name)?;
        let digest = base32(&hmac(&self.name, &[folder_id, name.as_bytes()])?);
        Ok(format!("{}.{}", &digest[..26], if kind == EntryKind::File { "f" } else { "d" }))
    }
    /// Header of a folder entry pointing to `child`.
    pub fn seal_folder(&self, parent: &[u8; 16], name: &str, modified_ms: u64, child: &[u8; 16], rng: &mut dyn RandomSource) -> Result<Vec<u8>> {
        let mut tail = [0u8; 62]; tail[..16].copy_from_slice(child);
        self.seal_entry(parent, name, EntryKind::Folder, modified_ms, &tail, rng)
    }
    /// Open an entry header found at `object` in `folder_id`; the object name is
    /// recomputed from the decrypted name and must match (no swaps/moves).
    pub fn open_entry(&self, folder_id: &[u8; 16], object: &str, header: &[u8]) -> Result<Entry> {
        let (entry, _) = self.open_entry_raw(folder_id, object, header)?;
        Ok(entry)
    }

    /// Encrypt `size` bytes from `input` into a file object (header + chunks).
    pub fn encrypt_file(&self, folder_id: &[u8; 16], file: NewFile<'_>, mut input: impl Read,
        output: &mut dyn Write, rng: &mut dyn RandomSource, checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<()> {
        let size = file.size;
        let mut writer = self.file_writer(folder_id, file, Box::new(output), rng)?;
        let mut buffer = Zeroizing::new(vec![0u8; CHUNK]);
        let mut consumed = 0u64;
        while consumed < size {
            checkpoint(consumed)?;
            let want = (size - consumed).min(CHUNK as u64) as usize;
            input.read_exact(&mut buffer[..want]).map_err(|_| CryptoError::Io)?;
            writer.write_all(&buffer[..want]).map_err(|_| CryptoError::Io)?;
            consumed += want as u64;
        }
        // The source must hold exactly `size` bytes.
        let mut extra = [0u8; 1];
        if input.read(&mut extra).map_err(|_| CryptoError::Io)? != 0 { return Err(CryptoError::InvalidInput); }
        writer.finish()?;
        checkpoint(consumed)
    }

    /// Streaming encryptor: writes the entry header now, then seals 64 KiB
    /// chunks as plaintext arrives; `finish` pads and seals the final chunk.
    /// Exactly `file.size` bytes must be written.
    pub fn file_writer<'w>(&self, folder_id: &[u8; 16], file: NewFile<'_>, mut output: Box<dyn Write + 'w>,
        rng: &mut dyn RandomSource) -> Result<FileWriter<'w>> {
        let NewFile { name, modified_ms, size } = file;
        let mut file_key = Zeroizing::new([0u8; 32]); rng.fill(file_key.as_mut())?;
        let mut file_id = [0u8; 16]; rng.fill(&mut file_id)?;
        let mut tail = Zeroizing::new([0u8; 62]);
        tail[..8].copy_from_slice(&size.to_le_bytes()); tail[8..40].copy_from_slice(file_key.as_ref());
        tail[40..56].copy_from_slice(&file_id); tail[56] = u8::from(self.padding);
        output.write_all(&self.seal_entry(folder_id, name, EntryKind::File, modified_ms, &tail, rng)?).map_err(|_| CryptoError::Io)?;
        let total = if self.padding { padme(size) } else { size };
        let cipher = XChaCha20Poly1305::new_from_slice(file_key.as_ref()).map_err(|_| CryptoError::InvalidInput)?;
        let mut prefix = [0u8; 15]; prefix.copy_from_slice(&file_id[..15]);
        Ok(FileWriter { output, cipher, prefix, ad: [self.vault_id.as_slice(), &file_id].concat(), size, total,
            chunks: total.div_ceil(CHUNK as u64).max(1), index: 0, written: 0, buffer: Zeroizing::new(Vec::with_capacity(CHUNK)) })
    }

    /// Decrypt a file object; plaintext is committed to `output` only after every
    /// chunk, the final-chunk flag, the padding and the end of input verified.
    pub fn decrypt_file(&self, folder_id: &[u8; 16], object: &str, mut input: impl Read, mut output: Box<dyn StreamSink>,
        max_size: u64, checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<Entry> {
        let result = (|| {
            let mut header = [0u8; ENTRY_HEADER_LEN];
            input.read_exact(&mut header).map_err(|_| CryptoError::Authentication)?;
            let (entry, tail) = self.open_entry_raw(folder_id, object, &header)?;
            let Entry::File { size, .. } = entry else { return Err(CryptoError::InvalidInput) };
            if size > max_size { return Err(CryptoError::LimitExceeded); }
            let padded = match tail[56] { 0 => false, 1 => true, _ => return Err(CryptoError::Authentication) };
            if tail[57..].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
            let total = if padded { padme(size) } else { size };
            let cipher = XChaCha20Poly1305::new_from_slice(&tail[8..40]).map_err(|_| CryptoError::InvalidInput)?;
            let mut prefix = [0u8; 15]; prefix.copy_from_slice(&tail[40..55]);
            let ad = [self.vault_id.as_slice(), &tail[40..56]].concat();
            let chunks = total.div_ceil(CHUNK as u64).max(1);
            let mut sealed = vec![0u8; CHUNK + 16];
            let mut written = 0u64;
            for index in 0..chunks {
                checkpoint(written)?;
                let len = (total - index * CHUNK as u64).min(CHUNK as u64) as usize;
                input.read_exact(&mut sealed[..len + 16]).map_err(|_| CryptoError::Authentication)?;
                let plain = Zeroizing::new(cipher.decrypt(XNonce::from_slice(&nonce(&prefix, index, index + 1 == chunks)),
                    Payload { msg: &sealed[..len + 16], aad: &ad }).map_err(|_| CryptoError::Authentication)?);
                let data_part = size.saturating_sub(index * CHUNK as u64).min(len as u64) as usize;
                if plain[data_part..].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
                output.write_all(&plain[..data_part]).map_err(|_| CryptoError::Io)?;
                written += data_part as u64;
            }
            let mut extra = [0u8; 1];
            if input.read(&mut extra).map_err(|_| CryptoError::Io)? != 0 { return Err(CryptoError::Authentication); }
            Ok(entry)
        })();
        match result {
            Ok(entry) => { output.commit()?; Ok(entry) }
            Err(error) => { let _ = output.abort(); Err(error) }
        }
    }

    /// Plan 4.7: random access to one file's content. Checks the header
    /// like `decrypt_file`; each chunk is then opened on its own (its index
    /// and the final-chunk flag are bound into the nonce, so a chunk cannot
    /// be moved or the file cut without failing authentication).
    pub fn file_cipher(&self, folder_id: &[u8; 16], object: &str, header: &[u8]) -> Result<(Entry, FileCipher)> {
        let (entry, tail) = self.open_entry_raw(folder_id, object, header)?;
        let Entry::File { size, .. } = entry else { return Err(CryptoError::InvalidInput) };
        let padded = match tail[56] { 0 => false, 1 => true, _ => return Err(CryptoError::Authentication) };
        if tail[57..].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
        let total = if padded { padme(size) } else { size };
        let cipher = XChaCha20Poly1305::new_from_slice(&tail[8..40]).map_err(|_| CryptoError::InvalidInput)?;
        let mut prefix = [0u8; 15]; prefix.copy_from_slice(&tail[40..55]);
        let ad = [self.vault_id.as_slice(), &tail[40..56]].concat();
        let chunks = total.div_ceil(CHUNK as u64).max(1);
        Ok((entry, FileCipher { cipher, prefix, ad, size, total, chunks }))
    }

    /// Re-issue an entry header under a new folder and/or name (rename/move).
    /// The content key, file id and size are kept, so the encrypted chunks that
    /// follow the header are reused byte for byte.
    pub fn reissue_header(&self, folder_id: &[u8; 16], object: &str, header: &[u8], to: &NewPlace<'_>,
        rng: &mut dyn RandomSource) -> Result<Vec<u8>> {
        let (entry, tail) = self.open_entry_raw(folder_id, object, header)?;
        let kind = match entry { Entry::File { .. } => EntryKind::File, Entry::Folder { .. } => EntryKind::Folder };
        self.seal_entry(to.folder, to.name, kind, to.modified_ms, &tail, rng)
    }

    fn seal_entry(&self, folder_id: &[u8; 16], name: &str, kind: EntryKind, modified_ms: u64, tail: &[u8; 62],
        rng: &mut dyn RandomSource) -> Result<Vec<u8>> {
        let normal = normalized(name)?;
        let object = self.entry_name(folder_id, name, kind)?;
        let mut payload = Zeroizing::new([0u8; PAYLOAD_LEN]);
        payload[..2].copy_from_slice(&(normal.len() as u16).to_le_bytes());
        payload[2..2 + normal.len()].copy_from_slice(normal.as_bytes());
        payload[258..266].copy_from_slice(&modified_ms.to_le_bytes());
        payload[266..].copy_from_slice(tail);
        let mut nonce = [0u8; 24]; rng.fill(&mut nonce)?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.file_header.as_ref()).map_err(|_| CryptoError::InvalidInput)?;
        let ad = entry_ad(&self.vault_id, folder_id, kind as u8, &object);
        let sealed = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: payload.as_ref(), aad: &ad }).map_err(|_| CryptoError::Authentication)?;
        let mut out = Vec::with_capacity(ENTRY_HEADER_LEN);
        out.extend_from_slice(ENTRY_MAGIC); out.push(1); out.push(kind as u8); out.extend_from_slice(&nonce); out.extend_from_slice(&sealed);
        Ok(out)
    }
    fn open_entry_raw(&self, folder_id: &[u8; 16], object: &str, header: &[u8]) -> Result<(Entry, Zeroizing<[u8; 62]>)> {
        if header.len() != ENTRY_HEADER_LEN || &header[..4] != ENTRY_MAGIC || header[4] != 1 { return Err(CryptoError::InvalidInput); }
        let kind = match header[5] { 1 => EntryKind::File, 2 => EntryKind::Folder, _ => return Err(CryptoError::InvalidInput) };
        let cipher = XChaCha20Poly1305::new_from_slice(self.file_header.as_ref()).map_err(|_| CryptoError::InvalidInput)?;
        let ad = entry_ad(&self.vault_id, folder_id, kind as u8, object);
        let payload = Zeroizing::new(cipher.decrypt(XNonce::from_slice(&header[6..30]), Payload { msg: &header[30..], aad: &ad })
            .map_err(|_| CryptoError::Authentication)?);
        let len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
        if len == 0 || len > NAME_MAX || payload[2 + len..258].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
        let name = std::str::from_utf8(&payload[2..2 + len]).map_err(|_| CryptoError::Authentication)?.to_owned();
        // The object must sit exactly where its own name places it.
        if self.entry_name(folder_id, &name, kind)? != object { return Err(CryptoError::Authentication); }
        let mut modified = [0u8; 8]; modified.copy_from_slice(&payload[258..266]);
        let modified_ms = u64::from_le_bytes(modified);
        let mut tail = Zeroizing::new([0u8; 62]); tail.copy_from_slice(&payload[266..]);
        let entry = match kind {
            EntryKind::File => {
                let mut size = [0u8; 8]; size.copy_from_slice(&tail[..8]);
                Entry::File { name, modified_ms, size: u64::from_le_bytes(size) }
            }
            EntryKind::Folder => {
                if tail[16..].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
                let mut folder_id = [0u8; 16]; folder_id.copy_from_slice(&tail[..16]);
                Entry::Folder { name, modified_ms, folder_id }
            }
        };
        Ok((entry, tail))
    }
}

/// See `VaultKeys::file_writer`.
pub struct FileWriter<'w> {
    output: Box<dyn Write + 'w>, cipher: XChaCha20Poly1305, prefix: [u8; 15], ad: Vec<u8>,
    size: u64, total: u64, chunks: u64, index: u64, written: u64, buffer: Zeroizing<Vec<u8>>,
}
impl FileWriter<'_> {
    fn seal(&mut self, last: bool) -> Result<()> {
        let sealed = self.cipher.encrypt(XNonce::from_slice(&nonce(&self.prefix, self.index, last)), Payload { msg: &self.buffer, aad: &self.ad })
            .map_err(|_| CryptoError::Authentication)?;
        self.output.write_all(&sealed).map_err(|_| CryptoError::Io)?;
        self.buffer.clear(); self.index += 1;
        Ok(())
    }
    /// Pad, seal the final chunk and check that exactly `size` bytes came in.
    pub fn finish(mut self) -> Result<()> {
        if self.written != self.size { return Err(CryptoError::InvalidInput); }
        let mut remaining = self.total - self.size;
        loop {
            let room = CHUNK - self.buffer.len();
            let fill = remaining.min(room as u64) as usize;
            let len = self.buffer.len(); self.buffer.resize(len + fill, 0); remaining -= fill as u64;
            let last = self.index + 1 == self.chunks;
            if last { if remaining != 0 { return Err(CryptoError::InvalidInput); } return self.seal(true); }
            if self.buffer.len() == CHUNK { self.seal(false)?; } else { return Err(CryptoError::InvalidInput); }
        }
    }
}
impl Write for FileWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() as u64 > self.size.saturating_sub(self.written) { return Err(std::io::Error::other("more data than declared")); }
        let mut rest = data;
        while !rest.is_empty() {
            let take = rest.len().min(CHUNK - self.buffer.len());
            self.buffer.extend_from_slice(&rest[..take]); rest = &rest[take..];
            // A full chunk is sealed now unless it is the final one (sealed by finish).
            if self.buffer.len() == CHUNK && self.index + 1 < self.chunks {
                self.seal(false).map_err(|_| std::io::Error::other("seal failed"))?;
            }
        }
        self.written += data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// Padmé (PURBs): pads to at most ~12 % with log-log bucket leakage.
pub fn padme(length: u64) -> u64 {
    if length < 2 { return length; }
    let e = 63 - length.leading_zeros() as u64;          // floor(log2 L)
    let s = 64 - e.leading_zeros() as u64;               // floor(log2 E) + 1
    let last_bits = e - s;
    let mask = (1u64 << last_bits) - 1;
    // The final bucket has no representable rounded boundary; cap it instead
    // of panicking in debug or wrapping to zero in release.
    length.checked_add(mask).map_or(u64::MAX, |rounded| rounded & !mask)
}

fn normalized(name: &str) -> Result<String> {
    let name: String = name.nfc().collect();
    if name.is_empty() || name.len() > NAME_MAX || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(CryptoError::InvalidInput);
    }
    Ok(name)
}
fn entry_ad(vault_id: &[u8; 16], folder_id: &[u8; 16], kind: u8, object: &str) -> Vec<u8> {
    [vault_id.as_slice(), folder_id, &[kind], object.as_bytes()].concat()
}
fn nonce(prefix: &[u8; 15], index: u64, last: bool) -> [u8; 24] {
    let mut n = [0u8; 24]; n[..15].copy_from_slice(prefix); n[15..23].copy_from_slice(&index.to_be_bytes()); n[23] = u8::from(last); n
}
fn base32(data: &[u8]) -> String { data_encoding::BASE32_NOPAD.encode(data).to_ascii_lowercase() }
fn hmac(key: &[u8; 32], parts: &[&[u8]]) -> Result<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| CryptoError::InvalidInput)?;
    for part in parts { mac.update(part); }
    Ok(mac.finalize().into_bytes().into())
}
fn hkdf(master: &[u8; 32], salt: &[u8], info: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let mut out = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(Some(salt), master).expand(info, out.as_mut()).map_err(|_| CryptoError::Derivation)?;
    Ok(out)
}
fn validate_password(password: &[u8]) -> Result<()> {
    if password.is_empty() || password.len() > 4096 { Err(CryptoError::InvalidInput) } else { Ok(()) }
}
fn password_kek(password: &[u8], header: &Header, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Zeroizing<[u8; 32]>> {
    let _permit = paranoic_crypto::kdf_gate::admit(u64::from(header.kdf.memory_kib), checkpoint)?;
    let params = argon2::Params::new(header.kdf.memory_kib, header.kdf.iterations, header.kdf.lanes, Some(32))
        .map_err(|_| CryptoError::Derivation)?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut kek = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(password, &header.salt, kek.as_mut()).map_err(|_| CryptoError::Derivation)?;
    Ok(kek)
}
fn recovery_kek(recovery: &[u8; 32], vault_id: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    hkdf(recovery, vault_id, b"paranoic/v1/recovery")
}
fn slot_ad(vault_id: &[u8; 16], kind: u8) -> Vec<u8> {
    [MAGIC.as_slice(), &VERSION.to_le_bytes(), vault_id, &[kind]].concat()
}
fn seal_slot(kek: &[u8; 32], master: &[u8; 32], vault_id: &[u8; 16], kind: SlotKind, rng: &mut dyn RandomSource)
    -> Result<(u8, [u8; 24], [u8; 48])> {
    let mut nonce = [0u8; 24]; rng.fill(&mut nonce)?;
    let cipher = XChaCha20Poly1305::new_from_slice(kek).map_err(|_| CryptoError::InvalidInput)?;
    let sealed = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: master, aad: &slot_ad(vault_id, kind as u8) })
        .map_err(|_| CryptoError::Authentication)?;
    let mut out = [0u8; 48]; out.copy_from_slice(&sealed);
    Ok((kind as u8, nonce, out))
}
fn open_with(header: &Header, bytes: &[u8], kek: &[u8; 32], kind: SlotKind) -> Result<VaultKeys> {
    let cipher = XChaCha20Poly1305::new_from_slice(kek).map_err(|_| CryptoError::InvalidInput)?;
    for (slot_kind, nonce, sealed) in header.slots.iter().filter(|s| s.0 == kind as u8) {
        let Ok(plain) = cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &slot_ad(&header.vault_id, *slot_kind) }) else { continue };
        let plain = Zeroizing::new(plain);
        let mut master = Zeroizing::new([0u8; 32]); master.copy_from_slice(&plain);
        verify_mac(header, bytes, &master)?;
        return derive_keys(&master, header);
    }
    Err(CryptoError::Authentication)
}
fn derive_keys(master: &[u8; 32], header: &Header) -> Result<VaultKeys> {
    let id = &header.vault_id;
    Ok(VaultKeys { vault_id: *id, padding: header.flags & 1 == 1, manifest_required: header.flags & 2 == 2,
        master: Zeroizing::new(*master), manifest: hkdf(master, id, b"paranoic/v1/manifest")?,
        file_header: hkdf(master, id, b"paranoic/v1/file-header")?, name: hkdf(master, id, b"paranoic/v1/name")?,
        folder: hkdf(master, id, b"paranoic/v1/folder")? })
}
fn verify_mac(header: &Header, bytes: &[u8], master: &[u8; 32]) -> Result<()> {
    let key = hkdf(master, &header.vault_id, b"paranoic/v1/header-mac")?;
    let body = &bytes[..bytes.len() - 32];
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref()).map_err(|_| CryptoError::InvalidInput)?;
    mac.update(body);
    mac.verify_slice(&bytes[bytes.len() - 32..]).map_err(|_| CryptoError::Authentication)
}
fn encode(header: &Header, master: &[u8; 32]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(FIXED_LEN + header.slots.len() * SLOT_LEN + 32);
    out.extend_from_slice(MAGIC); out.extend_from_slice(&VERSION.to_le_bytes()); out.extend_from_slice(&header.vault_id);
    out.extend_from_slice(&header.flags.to_le_bytes());
    for v in [header.kdf.memory_kib, header.kdf.iterations, header.kdf.lanes] { out.extend_from_slice(&v.to_le_bytes()); }
    out.extend_from_slice(&header.salt);
    out.push(header.slots.len() as u8);
    for (kind, nonce, sealed) in &header.slots { out.push(*kind); out.extend_from_slice(nonce); out.extend_from_slice(sealed); out.push(0); }
    let key = hkdf(master, &header.vault_id, b"paranoic/v1/header-mac")?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref()).map_err(|_| CryptoError::InvalidInput)?;
    mac.update(&out);
    out.extend_from_slice(&mac.finalize().into_bytes());
    Ok(out)
}
fn decode(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < FIXED_LEN + SLOT_LEN + 32 || &bytes[..8] != MAGIC { return Err(CryptoError::InvalidInput); }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    if u16_at(8) != VERSION { return Err(CryptoError::Unsupported); }
    let mut vault_id = [0u8; 16]; vault_id.copy_from_slice(&bytes[10..26]);
    let flags = u16_at(26);
    if flags & !3 != 0 { return Err(CryptoError::Unsupported); }
    let kdf = KdfParams { memory_kib: u32_at(28), iterations: u32_at(32), lanes: u32_at(36) };
    kdf.validate()?;
    let mut salt = [0u8; 32]; salt.copy_from_slice(&bytes[40..72]);
    let count = bytes[72] as usize;
    if !(1..=4).contains(&count) || bytes.len() != FIXED_LEN + count * SLOT_LEN + 32 { return Err(CryptoError::InvalidInput); }
    let mut slots = Vec::with_capacity(count);
    for i in 0..count {
        let s = &bytes[FIXED_LEN + i * SLOT_LEN..FIXED_LEN + (i + 1) * SLOT_LEN];
        if !(1..=2).contains(&s[0]) || s[73] != 0 { return Err(CryptoError::InvalidInput); }
        let mut nonce = [0u8; 24]; nonce.copy_from_slice(&s[1..25]);
        let mut sealed = [0u8; 48]; sealed.copy_from_slice(&s[25..73]);
        slots.push((s[0], nonce, sealed));
    }
    Ok(Header { vault_id, flags, kdf, salt, slots })
}

/// Opens single chunks of one file (`VaultKeys::file_cipher`).
pub struct FileCipher { cipher: XChaCha20Poly1305, prefix: [u8; 15], ad: Vec<u8>, size: u64, total: u64, chunks: u64 }
impl FileCipher {
    pub const CHUNK: usize = CHUNK;
    /// Plaintext length of the file.
    pub fn size(&self) -> u64 { self.size }
    pub fn chunks(&self) -> u64 { self.chunks }
    /// Exact length of the whole object: header + every sealed chunk.
    pub fn object_len(&self) -> u64 { ENTRY_HEADER_LEN as u64 + self.total + self.chunks * 16 }
    /// Where chunk `index` starts in the object, and its sealed length.
    pub fn chunk_range(&self, index: u64) -> (u64, usize) {
        let len = (self.total - index * CHUNK as u64).min(CHUNK as u64) as usize;
        (ENTRY_HEADER_LEN as u64 + index * (CHUNK as u64 + 16), len + 16)
    }
    /// The file's bytes in chunk `index` (padding verified and removed).
    pub fn open_chunk(&self, index: u64, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if index >= self.chunks || sealed.len() != self.chunk_range(index).1 { return Err(CryptoError::Authentication); }
        let mut plain = Zeroizing::new(self.cipher.decrypt(XNonce::from_slice(&nonce(&self.prefix, index, index + 1 == self.chunks)),
            Payload { msg: sealed, aad: &self.ad }).map_err(|_| CryptoError::Authentication)?);
        let data = self.size.saturating_sub(index * CHUNK as u64).min(plain.len() as u64) as usize;
        if plain[data..].iter().any(|b| *b != 0) { return Err(CryptoError::Authentication); }
        plain.truncate(data);
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    #[test] fn padme_matches_the_paper() {
        // Values from the PURBs paper's definition: small sizes exact, ≤ 12 % overhead.
        for (input, output) in [(0, 0), (1, 1), (2, 2), (9, 10), (100, 104), (1000, 1024), (65_537, 67_584)] {
            assert_eq!(padme(input), output, "{input}");
        }
        for l in [10u64, 1_000_003, 5_000_000_000] { assert!(padme(l) >= l && padme(l) - l <= l / 8); }
    }

    struct Sink(std::rc::Rc<std::cell::RefCell<(Vec<u8>, bool)>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.borrow_mut().0.extend_from_slice(b); Ok(b.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    impl StreamSink for Sink {
        fn commit(self: Box<Self>) -> Result<()> { self.0.borrow_mut().1 = true; Ok(()) }
        fn abort(self: Box<Self>) -> Result<()> { Ok(()) }
    }

    /// Padding must decrypt to zeros: a chunk re-sealed with the right file key
    /// but non-zero padding (data smuggled past the declared size) is refused.
    #[test] fn non_zero_padding_is_refused() {
        let kdf = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };
        let (_, keys) = create_vault(b"pw", kdf, true, &mut OsRandom, &mut || Ok(())).unwrap();
        let root = keys.vault_id;
        let object = keys.entry_name(&root, "p", EntryKind::File).unwrap();
        let mut sealed = Vec::new();
        keys.encrypt_file(&root, NewFile { name: "p", modified_ms: 0, size: 9 }, &b"123456789"[..], &mut sealed, &mut OsRandom, &mut |_| Ok(())).unwrap();
        let (_, tail) = keys.open_entry_raw(&root, &object, &sealed[..ENTRY_HEADER_LEN]).unwrap();
        let cipher = XChaCha20Poly1305::new_from_slice(&tail[8..40]).unwrap();
        let mut prefix = [0u8; 15]; prefix.copy_from_slice(&tail[40..55]);
        let ad = [keys.vault_id.as_slice(), &tail[40..56]].concat();
        let mut plain = cipher.decrypt(XNonce::from_slice(&nonce(&prefix, 0, true)), Payload { msg: &sealed[ENTRY_HEADER_LEN..], aad: &ad }).unwrap();
        assert_eq!(plain.len() as u64, padme(9));
        *plain.last_mut().unwrap() = 0x41;
        let forged = cipher.encrypt(XNonce::from_slice(&nonce(&prefix, 0, true)), Payload { msg: &plain, aad: &ad }).unwrap();
        let bytes = [&sealed[..ENTRY_HEADER_LEN], &forged[..]].concat();
        let out = std::rc::Rc::new(std::cell::RefCell::new((Vec::new(), false)));
        assert!(keys.decrypt_file(&root, &object, bytes.as_slice(), Box::new(Sink(out.clone())), u64::MAX, &mut |_| Ok(())).is_err());
        assert!(!out.borrow().1, "padding smuggle released");
    }
}
