//! Work with files inside an open vault (Sol's Rust requests, 2026-11-15):
//! move a file in with a verified copy before the source goes (E5.1), add a
//! shared Android document from its stream (ACTION_SEND), and — in later
//! parts — read and edit inside the vault without plaintext files (E4.7).
use crate::*;
use features::{finish, now_ms, run_wait};
use paranoic_core::{EntryKind, ErrorCode};
use paranoic_formats::{paranoic_v1::NewFile, paranoic_vault::Spool};
use std::io::{Read, Write};

/// A document handed to the app from outside (Android "Share" into Paranoic):
/// read once, front to back. `read(max)`: up to `max` bytes; empty = end.
/// Status: 0 ok, 1 permission, 2 unsupported, 3 IO.
#[uniffi::export(with_foreign)]
pub trait IncomingStream: Send + Sync {
    fn read(&self, max: u32) -> HostBytes;
}
struct Incoming<'a> { stream: &'a dyn IncomingStream, left: u64 }
impl Read for Incoming<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() { return Ok(0); }
        let got = self.stream.read(out.len().min(1 << 20) as u32);
        if got.error != 0 || got.data.len() > out.len() { return Err(std::io::Error::other("incoming document unreadable")); }
        if got.data.len() as u64 > self.left { return Err(std::io::Error::other("incoming document longer than declared")); }
        self.left -= got.data.len() as u64;
        out[..got.data.len()].copy_from_slice(&got.data);
        Ok(got.data.len())
    }
}
/// BLAKE3 of plaintext read back from the vault (memory only, nothing kept).
struct HashSink(Option<std::rc::Rc<std::cell::RefCell<blake3::Hasher>>>);
impl Write for HashSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if let Some(h) = &self.0 { h.borrow_mut().update(data); }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl paranoic_crypto::StreamSink for HashSink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}

/// Result of moving a file into a vault. `source_removed` false: the copy is
/// in the vault and verified, but the source changed or could not be
/// removed — it is kept (never deleted on doubt).
#[derive(Clone, uniffi::Record)]
pub struct NativeMoveIn { pub bytes: u64, pub source_removed: bool }

impl NativeRuntime {
    fn vault_file_hash(&self, handle: &str, path: &[String], name: &str) -> Result<[u8; 32]> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        let hasher = std::rc::Rc::new(std::cell::RefCell::new(blake3::Hasher::new()));
        vault.read_file(&self.core, &folder, name, Box::new(HashSink(Some(hasher.clone()))), u64::MAX, &mut |_| Ok(()))?;
        let digest = *hasher.borrow().finalize().as_bytes();
        Ok(digest)
    }
    fn source_hash(&self, source: &Location) -> Result<(paranoic_core::Metadata, [u8; 32])> {
        let mut input = self.core.open_read(source)?;
        let meta = input.metadata()?;
        if meta.kind != EntryKind::File { return Err(BridgeError::Unsupported); }
        let mut hasher = blake3::Hasher::new();
        let mut buffer = zeroize::Zeroizing::new(vec![0u8; 1 << 16]);
        loop {
            let n = input.read(&mut buffer).map_err(|_| BridgeError::Unavailable)?;
            if n == 0 { break; }
            hasher.update(&buffer[..n]);
        }
        if input.metadata()? != meta { return Err(BridgeError::Changed); }
        Ok((meta, *hasher.finalize().as_bytes()))
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// «Переместить в…» a vault: the file is added (or found already added by
    /// an earlier attempt with identical content), the vault copy is decrypted
    /// and compared with the source, and only then the source — the exact
    /// version just read — is deleted by a Core job with `request_id` (a lost
    /// reply is checked in the journal, never retried with a new ID). The job
    /// pins the source and checks its content hash through the pin (У-69);
    /// storages that cannot pin keep the source (`source_removed = false`).
    pub fn vault_move_in(&self, handle: String, path: Vec<String>, source: NativeLocation, request_id: String) -> Result<NativeMoveIn> {
        let source = source.core()?;
        let name = source.path.parent_and_name()?.1.to_owned();
        let (meta, digest) = self.source_hash(&source)?;
        let present = {
            let vault = self.vault(&handle)?;
            let folder = vault.resolve(&self.core, &path)?;
            match vault.stat(&self.core, &folder, &name, paranoic_formats::paranoic_v1::EntryKind::File) {
                Ok(_) => true,
                Err(error) if error.code == ErrorCode::NotFound => false,
                Err(error) => return Err(error.into()),
            }
        };
        // Already there from an earlier attempt: only identical content counts.
        if present && self.vault_file_hash(&handle, &path, &name)? != digest { return Err(BridgeError::Conflict); }
        if !present { self.vault_add(handle.clone(), path.clone(), source.clone().into())?; }
        if self.vault_file_hash(&handle, &path, &name)? != digest { return Err(BridgeError::Changed); }
        // The copy is verified; remove the source only if it is still that version.
        let removed = (|| -> Result<()> {
            let job = self.core.submit_with_id(paranoic_core::JobId(Uuid::parse_str(&request_id).map_err(|_| BridgeError::Invalid)?),
                // У-69: Core pins the source, re-verifies size/time and BLAKE3
                // through the pin and removes only that object.
                Operation::DeleteFileContent { target: source.clone(), expected: meta.clone(), expected_blake3: digest })?;
            finish(run_wait(&self.core, job.id)?, 1)
        })();
        match removed {
            Ok(()) => Ok(NativeMoveIn { bytes: meta.bytes, source_removed: true }),
            Err(BridgeError::Invalid) => Err(BridgeError::Invalid),
            Err(_) => Ok(NativeMoveIn { bytes: meta.bytes, source_removed: false }),
        }
    }
    /// «Поделиться → Paranoic»: encrypt a document read from `stream` straight
    /// into the vault (only ciphertext is spooled). `size` must be exact; a
    /// stream that ends early or runs long is refused and nothing is added.
    pub fn vault_add_stream(&self, handle: String, path: Vec<String>, name: String, size: u64, modified_ms: Option<u64>,
        stream: Arc<dyn IncomingStream>) -> Result<()> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let spool = Self::spool_path()?;
        let mut input = Incoming { stream: stream.as_ref(), left: size };
        let prepared = vault.prepare_file(&folder, NewFile { name: &name, modified_ms: modified_ms.unwrap_or_else(now_ms), size },
            &mut input, Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()));
        let result = prepared.map_err(BridgeError::from).and_then(|p| {
            // Nothing beyond the declared size may follow.
            if input.left != 0 || !stream.read(1).data.is_empty() { return Err(BridgeError::Changed); }
            self.run_vault(&handle, &vault, vault.plan_add(&p), &spool)
        });
        self.drop_spool(&spool);
        result
    }
}


// ------------------------------------------------- E4.7: inside the vault

use paranoic_formats::paranoic_vault::VaultFileReader;
use paranoic_documents::{ArchiveEntry, ArchiveLimits, CodecLimits, IsolatedSevenZip, TextDocument, ZipService};

const STREAMS: usize = 8;
/// Open vault file streams: (id, vault handle, reader). Emptied by Quick Lock.
/// A file opened for random-access reading inside a 1.8 or Cryptomator vault.
pub(crate) trait RandomRead: Send {
    fn size(&self) -> u64;
    fn read_at(&mut self, offset: u64, len: usize) -> Result<zeroize::Zeroizing<Vec<u8>>>;
}
impl RandomRead for VaultFileReader {
    fn size(&self) -> u64 { VaultFileReader::size(self) }
    fn read_at(&mut self, offset: u64, len: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> { Ok(VaultFileReader::read_at(self, offset, len)?) }
}
impl RandomRead for paranoic_formats::CryptomatorFileReader {
    fn size(&self) -> u64 { paranoic_formats::CryptomatorFileReader::size(self) }
    fn read_at(&mut self, offset: u64, len: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        paranoic_formats::CryptomatorFileReader::read_at(self, offset, len).map_err(crate::crypto_files::crypto_error)
    }
}
/// Which kind of vault handle a stream belongs to (checked on every read).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VaultKind { Paranoic, Cryptomator }
type OpenStream = (String, String, VaultKind, Arc<Mutex<Box<dyn RandomRead>>>);
static STREAMS_OPEN: Mutex<Vec<OpenStream>> = Mutex::new(Vec::new());
pub(crate) fn forget_streams() { if let Ok(mut s) = STREAMS_OPEN.lock() { s.clear(); } }
/// Locking one vault ends its open streams.
pub(crate) fn forget_streams_of(handle: &str) { if let Ok(mut s) = STREAMS_OPEN.lock() { s.retain(|(_, h, _, _)| h != handle); } }
pub(crate) fn register_stream(handle: String, kind: VaultKind, reader: Box<dyn RandomRead>) -> Result<NativeVaultStream> {
    let id = Uuid::new_v4().to_string();
    let size = reader.size();
    let mut open = STREAMS_OPEN.lock().map_err(lock_error)?;
    if open.len() >= STREAMS { open.remove(0); }
    open.push((id.clone(), handle, kind, Arc::new(Mutex::new(reader))));
    Ok(NativeVaultStream { id, size })
}

/// A vault file opened for reading in memory (viewers: PDF bytes over a
/// channel, images, media data source). `size`: plaintext length.
#[derive(Clone, uniffi::Record)]
pub struct NativeVaultStream { pub id: String, pub size: u64 }
/// A text inside a vault. `version_token`: strong version of the encrypted
/// object (BLAKE3 of its ciphertext, hex) — the only thing save accepts (У-70).
/// `version_bytes`/`version_modified_ms` are informational.
#[derive(Clone, uniffi::Record)]
pub struct NativeVaultText { pub text: String, pub encoding: u8, pub version_bytes: u64, pub version_modified_ms: Option<u64>,
    pub version_token: String }

/// Plaintext collected in memory only (wiped on drop).
pub(crate) struct MemorySink(pub(crate) Option<std::rc::Rc<std::cell::RefCell<zeroize::Zeroizing<Vec<u8>>>>>, pub(crate) usize);
impl Write for MemorySink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let Some(buffer) = &self.0 else { return Err(std::io::Error::other("closed")) };
        if buffer.borrow().len() + data.len() > self.1 { return Err(std::io::Error::other("text too large")); }
        buffer.borrow_mut().extend_from_slice(data); Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl paranoic_crypto::StreamSink for MemorySink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}
/// Read + Seek over a vault file (archive libraries inside the vault).
pub(crate) struct VaultSeek { pub(crate) reader: Box<dyn RandomRead>, pub(crate) position: u64 }
impl Read for VaultSeek {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let data = self.reader.read_at(self.position, out.len().min(1 << 20)).map_err(|_| std::io::Error::other("vault read failed"))?;
        out[..data.len()].copy_from_slice(&data);
        self.position += data.len() as u64;
        Ok(data.len())
    }
}
impl std::io::Seek for VaultSeek {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let size = self.reader.size();
        let next = match to {
            std::io::SeekFrom::Start(p) => Some(p),
            std::io::SeekFrom::End(d) => size.checked_add_signed(d),
            std::io::SeekFrom::Current(d) => self.position.checked_add_signed(d),
        }.ok_or_else(|| std::io::Error::other("seek out of range"))?;
        self.position = next; Ok(next)
    }
}

impl NativeRuntime {
    pub(crate) fn vault_bytes(&self, handle: &str, path: &[String], name: &str, limit: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        let buffer = std::rc::Rc::new(std::cell::RefCell::new(zeroize::Zeroizing::new(Vec::new())));
        vault.read_file(&self.core, &folder, name, Box::new(MemorySink(Some(buffer.clone()), limit)), limit as u64, &mut |_| Ok(()))?;
        let bytes = std::mem::take(&mut *buffer.borrow_mut());
        Ok(bytes)
    }
    /// Encrypt `bytes` as `name` in the vault folder: new, or replacing only `expected`.
    fn vault_put(&self, handle: &str, path: &[String], name: &str, bytes: &[u8], expected: Option<(paranoic_core::Metadata, [u8; 32])>) -> Result<()> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        let spool = Self::spool_path()?;
        let prepared = vault.prepare_file(&folder, NewFile { name, modified_ms: now_ms(), size: bytes.len() as u64 }, bytes,
            Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()));
        let result = prepared.map_err(BridgeError::from).and_then(|p| {
            let plan = match expected { Some((m, digest)) => vault.plan_replace_content(&p, m, digest), None => vault.plan_add(&p) };
            self.run_vault(handle, &vault, plan, &spool)
        });
        self.drop_spool(&spool);
        result
    }
    fn vault_object_version(&self, handle: &str, path: &[String], name: &str) -> Result<(paranoic_core::Metadata, [u8; 32])> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        Ok(vault.object_version(&self.core, &folder, name)?)
    }
    pub(crate) fn vault_stat(&self, handle: &str, path: &[String], name: &str) -> Result<paranoic_core::Metadata> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        Ok(vault.stat(&self.core, &folder, name, paranoic_formats::paranoic_v1::EntryKind::File)?)
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Open a vault file for random-access reading in memory (≤ 8 at once;
    /// the oldest closes). Quick Lock or locking the vault closes them all.
    pub fn vault_stream_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeVaultStream> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let reader = vault.open_reader(&self.core, &folder, &name)?;
        register_stream(handle, VaultKind::Paranoic, Box::new(reader))
    }
    /// Up to `len` (≤ 1 MiB) plaintext bytes from `offset`; fewer at the end.
    /// Works for streams of both 1.8 and Cryptomator vaults.
    pub fn vault_stream_read(&self, id: String, offset: u64, len: u32) -> Result<Vec<u8>> {
        if len > 1 << 20 { return Err(BridgeError::Limit); }
        let (handle, kind, reader) = {
            let open = STREAMS_OPEN.lock().map_err(lock_error)?;
            let (_, h, k, r) = open.iter().find(|(i, _, _, _)| *i == id).ok_or(BridgeError::Locked)?;
            (h.clone(), *k, Arc::clone(r))
        };
        // The vault is still open.
        match kind { VaultKind::Paranoic => { self.vault(&handle)?; } VaultKind::Cryptomator => { self.cryptomator(&handle)?; } }
        let data = reader.lock().map_err(lock_error)?.read_at(offset, len as usize)?;
        Ok(data.to_vec())
    }
    pub fn vault_stream_close(&self, id: String) {
        if let Ok(mut open) = STREAMS_OPEN.lock() { open.retain(|(i, _, _, _)| *i != id); }
    }

    /// A text file inside the vault, decrypted in memory (≤ 16 MiB).
    pub fn vault_text_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeVaultText> {
        let (version, digest) = self.vault_object_version(&handle, &path, &name)?;
        let bytes = self.vault_bytes(&handle, &path, &name, crate::documents::TEXT_LIMIT)?;
        let document = TextDocument::open(&bytes[..], crate::documents::TEXT_LIMIT, || Ok(()))?;
        // The plaintext read must belong to the version reported.
        if self.vault_object_version(&handle, &path, &name)? != (version.clone(), digest) { return Err(BridgeError::Changed); }
        Ok(NativeVaultText { text: document.text().to_owned(), encoding: crate::documents::encoding_code(document.encoding()),
            version_bytes: version.bytes, version_modified_ms: version.modified_unix_ms, version_token: hex(&digest) })
    }
    /// Save `text` over the vault file, keeping its encoding — only over the
    /// exact encrypted object `version_token` from `vault_text_open` (else
    /// `Changed`, nothing written). Core checks the token again when the
    /// replace job runs, so a write between check and replace is not lost (У-70).
    pub fn vault_text_save(&self, handle: String, path: Vec<String>, name: String, version_token: String, text: String) -> Result<()> {
        let text = zeroize::Zeroizing::new(text);
        let (current, digest) = self.vault_object_version(&handle, &path, &name)?;
        if hex(&digest) != version_token { return Err(BridgeError::Changed); }
        let original = self.vault_bytes(&handle, &path, &name, crate::documents::TEXT_LIMIT)?;
        if self.vault_object_version(&handle, &path, &name)? != (current.clone(), digest) { return Err(BridgeError::Changed); }
        let mut document = TextDocument::open(&original[..], crate::documents::TEXT_LIMIT, || Ok(()))?;
        let length = document.text().len();
        document.replace(document.revision(), 0..length, &text)?;
        let encoded = std::rc::Rc::new(std::cell::RefCell::new(zeroize::Zeroizing::new(Vec::new())));
        document.save(Box::new(MemorySink(Some(encoded.clone()), crate::documents::TEXT_LIMIT + 4)), || Ok(()))?;
        let bytes = std::mem::take(&mut *encoded.borrow_mut());
        self.vault_put(&handle, &path, &name, &bytes, Some((current, digest)))
    }
    /// A NEW UTF-8 text file inside the vault (`Conflict` if the name is taken).
    pub fn vault_text_create(&self, handle: String, path: Vec<String>, name: String, text: String) -> Result<()> {
        let text = zeroize::Zeroizing::new(text);
        if text.len() > crate::documents::TEXT_LIMIT || text.contains('\0') { return Err(BridgeError::Invalid); }
        match self.vault_stat(&handle, &path, &name) {
            Ok(_) => return Err(BridgeError::Conflict),
            Err(BridgeError::Unavailable) => {}
            Err(error) => return Err(error),
        }
        self.vault_put(&handle, &path, &name, text.as_bytes(), None)
    }

    /// Entries of an archive stored in the vault (`kind`: ARCHIVE_ZIP or
    /// ARCHIVE_7Z; read in memory; 7z decoded in the confined worker).
    pub fn vault_archive_list(&self, handle: String, path: Vec<String>, name: String, kind: u8, password: Option<String>) -> Result<Vec<NativeArchiveEntry>> {
        self.archive_list_from(self.vault_seek(&handle, &path, &name)?, kind, password)
    }
    /// Extract a ZIP stored in the vault into a NEW folder `target_name` in
    /// the vault folder `target_path`: every file is encrypted straight from
    /// the archive stream — no plaintext file anywhere. A failure part-way
    /// leaves the new folder partly filled (it can be deleted).
    #[allow(clippy::too_many_arguments)] // flat UniFFI call
    pub fn vault_archive_extract(&self, handle: String, path: Vec<String>, name: String, kind: u8, password: Option<String>,
        target_path: Vec<String>, target_name: String) -> Result<u64> {
        let source = self.vault_seek(&handle, &path, &name)?;
        let mkdir = |parent: &[String], name: &str| self.vault_mkdir(handle.clone(), parent.to_vec(), name.to_owned());
        let add = |parent: &[String], name: &str, size: u64, input: &mut dyn Read| -> Result<()> {
            let vault = self.vault(&handle)?;
            let inside = vault.resolve(&self.core, parent)?;
            let spool = Self::spool_path()?;
            let prepared = vault.prepare_file(&inside, NewFile { name, modified_ms: now_ms(), size }, input,
                Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()));
            let result = prepared.map_err(BridgeError::from).and_then(|p| self.run_vault(&handle, &vault, vault.plan_add(&p), &spool));
            self.drop_spool(&spool);
            result
        };
        self.archive_extract_into(source, kind, password, target_path, target_name, &mkdir, &add)
    }
}

/// Folder creation / file encryption in the vault an archive is extracted into.
pub(crate) type MakeDir<'a> = dyn Fn(&[String], &str) -> Result<()> + 'a;
pub(crate) type AddFile<'a> = dyn Fn(&[String], &str, u64, &mut dyn Read) -> Result<()> + 'a;

impl NativeRuntime {
    pub(crate) fn archive_list_from(&self, source: VaultSeek, kind: u8, password: Option<String>) -> Result<Vec<NativeArchiveEntry>> {
        let password = password.filter(|p| !p.is_empty()).map(zeroize::Zeroizing::new);
        let entries: Vec<ArchiveEntry> = match kind {
            ARCHIVE_ZIP => ZipService::open(source, ArchiveLimits::default(), || Ok(()))?.entries().to_vec(),
            ARCHIVE_7Z => self.vault_sevenz(source, password)?.entries().to_vec(),
            _ => return Err(BridgeError::Invalid),
        };
        Ok(entries.iter().map(|e| NativeArchiveEntry { path: e.path.as_str().to_owned(), folder: e.directory, size: e.bytes,
            compressed: e.compressed_bytes, encrypted: e.encrypted }).collect())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn archive_extract_into(&self, source: VaultSeek, kind: u8, password: Option<String>,
        target_path: Vec<String>, target_name: String, mkdir: &MakeDir<'_>, add: &AddFile<'_>) -> Result<u64> {
        let password = password.filter(|p| !p.is_empty()).map(zeroize::Zeroizing::new);
        enum Opened { Zip(ZipService<VaultSeek>), SevenZ(IsolatedSevenZip<VaultSeek>) }
        let mut opened = match kind {
            ARCHIVE_ZIP => Opened::Zip(ZipService::open(source, ArchiveLimits::default(), || Ok(()))?),
            ARCHIVE_7Z => Opened::SevenZ(self.vault_sevenz(source, password.clone())?),
            _ => return Err(BridgeError::Invalid),
        };
        let entries: Vec<ArchiveEntry> = match &opened { Opened::Zip(z) => z.entries().to_vec(), Opened::SevenZ(z) => z.entries().to_vec() };
        // Password problems surface before the new folder is made.
        if paranoic_documents::any_encrypted(&entries) && password.is_none() { return Err(BridgeError::PasswordRequired); }
        // ...including a wrong one: checked against the archive itself (audit 24).
        if let Opened::Zip(zip) = &mut opened { zip.check_password(password.as_ref())?; }
        mkdir(&target_path, &target_name)?;
        let mut root = target_path; root.push(target_name);
        let made = std::cell::RefCell::new(std::collections::BTreeSet::new());
        // Folders for `relative` (and its parents) under the root, made once.
        let ensure = |relative: &str| -> Result<()> {
            let mut path = root.clone();
            for part in relative.split('/').filter(|p| !p.is_empty()) {
                if made.borrow_mut().insert(format!("{}/{part}", path.join("/"))) { mkdir(&path, part)?; }
                path.push(part.to_owned());
            }
            Ok(())
        };
        for entry in entries.iter().filter(|e| e.directory) { ensure(entry.path.as_str())?; }
        let mut files = 0u64;
        let mut failure: Option<BridgeError> = None;
        let mut put = |entry: &ArchiveEntry, input: &mut dyn Read| {
            let run = (|| -> Result<()> {
                let parts: Vec<&str> = entry.path.as_str().split('/').collect();
                let (dirs, file) = parts.split_at(parts.len() - 1);
                ensure(&dirs.join("/"))?;
                let mut target = root.clone(); target.extend(dirs.iter().map(|d| d.to_string()));
                add(&target, file[0], entry.bytes, input)
            })();
            match run { Ok(()) => { files += 1; Ok(()) } Err(e) => { failure = Some(e); Err(paranoic_core::CoreError::new(ErrorCode::Cancelled, "stopped")) } }
        };
        let walked = match opened {
            Opened::Zip(mut zip) => zip.for_each_file(password, &mut put),
            Opened::SevenZ(sevenz) => sevenz.for_each_file(&mut put),
        };
        if let Some(error) = failure { return Err(error); }
        walked?;
        Ok(files)
    }
}

impl NativeRuntime {
    fn vault_seek(&self, handle: &str, path: &[String], name: &str) -> Result<VaultSeek> {
        let vault = self.vault(handle)?;
        let folder = vault.resolve(&self.core, path)?;
        Ok(VaultSeek { reader: Box::new(vault.open_reader(&self.core, &folder, name)?), position: 0 })
    }
    fn vault_sevenz(&self, source: VaultSeek, password: Option<zeroize::Zeroizing<String>>) -> Result<IsolatedSevenZip<VaultSeek>> {
        Ok(IsolatedSevenZip::open_with_host(source, CodecLimits::default(), password, &self.pdf_worker,
            Arc::new(std::sync::atomic::AtomicBool::new(false)), self.document_worker_host()?, || Ok(()))?)
    }
}

pub(crate) fn hex(digest: &[u8; 32]) -> String { digest.iter().map(|b| format!("{b:02x}")).collect() }
