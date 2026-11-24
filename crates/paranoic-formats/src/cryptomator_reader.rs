//! Read-only vault access over the core's confined storage grants.
use std::{io::{self, Read}, sync::Arc, time::Duration};
use paranoic_core::{Storage, StorageDomain, ResourcePath, ReadHandle, Metadata, EntryKind};
use paranoic_crypto::{CryptoError, Result, StreamSink, LifecycleEvent};
use zeroize::Zeroizing;
use crate::{CryptomatorVault, CryptomatorSession, CryptomatorDirectoryId,
    CryptomatorContentLimits, CryptomatorContentOutcome, CryptomatorLengthPolicy};

/// Owns the grant and session; decrypted objects never enter StorageRegistry.
/// No writes, symlink traversal, mounts or external key resolution.
pub struct CryptomatorReader {
    pub(crate) storage: Arc<dyn Storage>,
    pub(crate) root: ResourcePath,
    pub(crate) session: Arc<CryptomatorSession>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptomatorListedKind { File, Directory, UnsupportedLink }

/// Decrypted name has no Debug/serde/Clone and is erased on Drop.
pub struct CryptomatorListedEntry {
    name: Zeroizing<String>,
    kind: CryptomatorListedKind,
    ciphertext_bytes: Option<u64>,
}
impl CryptomatorListedEntry {
    pub fn name(&self) -> &str { &self.name }
    pub fn kind(&self) -> CryptomatorListedKind { self.kind }
    /// Unauthenticated storage metadata, not plaintext length.
    pub fn ciphertext_bytes(&self) -> Option<u64> { self.ciphertext_bytes }
}

pub struct CryptomatorListing {
    entries: Vec<CryptomatorListedEntry>,
    ignored_objects: usize,
}
impl CryptomatorListing {
    pub fn entries(&self) -> &[CryptomatorListedEntry] { &self.entries }
    /// Foreign/maintenance objects excluded from the visible namespace.
    pub fn ignored_objects(&self) -> usize { self.ignored_objects }
}

impl CryptomatorReader {
    pub fn open<F: FnMut() -> Result<()>>(
        storage: Arc<dyn Storage>, root: ResourcePath, password: Zeroizing<String>,
        lifetime: Duration, mut checkpoint: F,
    ) -> Result<Self> {
        if storage.domain() != StorageDomain::Files || !storage.capabilities().read {
            return Err(CryptoError::Permissions);
        }
        checkpoint()?;
        require_directory(storage.as_ref(), &root)?;
        let keys = read_small(storage.as_ref(), &join(&root, "masterkey.cryptomator")?, 16384, &mut checkpoint)?;
        let config_path = join(&root, "vault.cryptomator")?;
        // Format 7 vaults have no vault.cryptomator; only a verified masterkey
        // "version": 7 is accepted on that path (see unlock_legacy).
        let vault = match storage.metadata(&config_path) {
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound =>
                CryptomatorVault::unlock_legacy(&keys, password, &mut checkpoint)?,
            Err(error) => return Err(storage_error(error)),
            Ok(_) => {
                let config = read_small(storage.as_ref(), &config_path, 16384, &mut checkpoint)?;
                let token = std::str::from_utf8(&config).map_err(|_| CryptoError::InvalidInput)?;
                CryptomatorVault::unlock(&keys, token, password, &mut checkpoint)?
            }
        };
        let session = Arc::new(CryptomatorSession::new(vault, lifetime)?);
        let reader = Self { storage, root, session };
        require_directory(reader.storage.as_ref(), &reader.data_path(&CryptomatorDirectoryId::root())?)?;
        checkpoint()?;
        Ok(reader)
    }

    pub fn request_lock(&self) { self.session.request_lock(); }
    pub fn lock(&self) { self.session.lock(); }
    pub fn expire(&self) -> bool { self.session.expire() }
    pub fn on_event(&self, event: LifecycleEvent) { self.session.on_event(event); }
    pub fn vault_id(&self) -> Result<uuid::Uuid> { self.session.id() }
    /// Vault format (7 or 8) and content cipher of the unlocked vault.
    pub fn format(&self) -> Result<u32> { self.session.format() }
    pub fn cipher_combo(&self) -> Result<crate::CryptomatorCipherCombo> { self.session.cipher_combo() }

    /// Complete bounded observation, not a paged or atomic snapshot. Empty
    /// components selects root. Corrupt encrypted entries fail the whole call.
    pub fn list_directory<F: FnMut() -> Result<()>>(
        &self, components: &[String], limit: usize, mut checkpoint: F,
    ) -> Result<CryptomatorListing> {
        if !(1..=4096).contains(&limit) { return Err(CryptoError::InvalidInput); }
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok(()) };
        let (directory, path) = self.resolve_directory(components, &mut check)?;
        check()?;
        let rows = self.storage.list(&path, limit).map_err(storage_error)?;
        if rows.len() > limit { return Err(CryptoError::LimitExceeded); }
        let mut entries = Vec::new();
        let mut ignored_objects = 0;
        let mut observed = std::collections::HashSet::new();
        for row in rows {
            check()?;
            let (parent, component) = row.location.path.parent_and_name().map_err(|_| CryptoError::InvalidInput)?;
            if parent != path || row.location.storage != self.storage.id()
                || !observed.insert(row.location.path.clone()) { return Err(CryptoError::InvalidInput); }
            if component == "dirid.c9r" || !(component.ends_with(".c9r") || component.ends_with(".c9s")) {
                ignored_objects += 1;
                continue;
            }
            let shortened = component.ends_with(".c9s");
            let mapping = if shortened {
                require_directory(self.storage.as_ref(), &row.location.path)?;
                Some(read_small(self.storage.as_ref(), &join(&row.location.path, "name.c9s")?, 368, &mut check)?)
            } else { None };
            let mapping_text = mapping.as_ref().map(|v| std::str::from_utf8(v))
                .transpose().map_err(|_| CryptoError::InvalidInput)?;
            let name = self.session.decrypt_name(&directory, component, mapping_text)?;
            let (kind, bytes) = match row.metadata.kind {
                EntryKind::File if !shortened => (CryptomatorListedKind::File, Some(row.metadata.bytes)),
                EntryKind::Directory => self.wrapper_kind(&row.location.path, shortened, &mut check)?,
                _ => return Err(CryptoError::Unsupported),
            };
            entries.push(CryptomatorListedEntry { name, kind, ciphertext_bytes: bytes });
        }
        entries.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        if entries.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(CryptoError::Authentication);
        }
        check()?;
        Ok(CryptomatorListing { entries, ignored_objects })
    }

    pub(crate) fn wrapper_kind<F: FnMut() -> Result<()>>(&self, path: &ResourcePath, shortened: bool, check: &mut F)
        -> Result<(CryptomatorListedKind, Option<u64>)>
    {
        let rows = self.storage.list(path, 4).map_err(storage_error)?;
        if rows.len() > 4 { return Err(CryptoError::LimitExceeded); }
        let mut kind = None;
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            check()?;
            let (parent, component) = row.location.path.parent_and_name().map_err(|_| CryptoError::InvalidInput)?;
            if parent != *path || row.location.storage != self.storage.id()
                || !seen.insert(component.to_owned()) || row.metadata.kind != EntryKind::File {
                return Err(CryptoError::InvalidInput);
            }
            let value = match component {
                "name.c9s" if shortened => continue,
                "contents.c9r" if shortened => (CryptomatorListedKind::File, Some(row.metadata.bytes)),
                "dir.c9r" => {
                    let link = read_small(self.storage.as_ref(), &row.location.path, 36, check)?;
                    let id = std::str::from_utf8(&link).map_err(|_| CryptoError::InvalidInput)?;
                    if id.is_empty() { return Err(CryptoError::InvalidInput); }
                    CryptomatorDirectoryId::parse(id)?;
                    (CryptomatorListedKind::Directory, None)
                }
                "symlink.c9r" => (CryptomatorListedKind::UnsupportedLink, None),
                _ => return Err(CryptoError::Unsupported),
            };
            if kind.replace(value).is_some() { return Err(CryptoError::InvalidInput); }
        }
        if shortened && !seen.contains("name.c9s") { return Err(CryptoError::InvalidInput); }
        kind.ok_or(CryptoError::InvalidInput)
    }

    pub(crate) fn resolve_directory<F: FnMut() -> Result<()>>(&self, components: &[String], check: &mut F)
        -> Result<(CryptomatorDirectoryId, ResourcePath)>
    {
        self.resolve_directory_excluding(components, None, check)
    }

    /// Compare resolved data identities, not plaintext prefixes: directory links
    /// can alias the same UUID under different names.
    pub(crate) fn resolve_directory_excluding<F: FnMut() -> Result<()>>(&self,
        components: &[String], forbidden: Option<&ResourcePath>, check: &mut F)
        -> Result<(CryptomatorDirectoryId, ResourcePath)>
    {
        if components.len() > 64 { return Err(CryptoError::LimitExceeded); }
        let mut directory = CryptomatorDirectoryId::root();
        let mut path = self.data_path(&directory)?;
        if forbidden == Some(&path) { return Err(CryptoError::InvalidInput); }
        let mut visited = vec![path.clone()];
        for name in components {
            check()?;
            let encoded = self.session.encrypt_name(&directory, name)?;
            let entry = join(&path, encoded.component())?;
            require_directory(self.storage.as_ref(), &entry)?;
            if let Some(expected) = encoded.name_mapping() {
                let mapping = read_small(self.storage.as_ref(), &join(&entry, "name.c9s")?, 368, check)?;
                if mapping.as_slice() != expected.as_bytes() { return Err(CryptoError::Authentication); }
            }
            let link = read_small(self.storage.as_ref(), &join(&entry, "dir.c9r")?, 36, check)?;
            let id = std::str::from_utf8(&link).map_err(|_| CryptoError::InvalidInput)?;
            if id.is_empty() { return Err(CryptoError::InvalidInput); }
            directory = CryptomatorDirectoryId::parse(id)?;
            path = self.data_path(&directory)?;
            if forbidden == Some(&path) { return Err(CryptoError::InvalidInput); }
            if visited.contains(&path) { return Err(CryptoError::InvalidInput); }
            visited.push(path.clone());
        }
        require_directory(self.storage.as_ref(), &path)?;
        check()?;
        Ok((directory, path))
    }

    /// Components are decrypted names, relative to the vault root. Traversal is
    /// bounded to 64 components, and known directory cycles are rejected.
    /// The explicit length policy is forwarded unchanged to the content codec.
    pub fn decrypt_file<F: FnMut(u64) -> Result<()>>(
        &self, components: &[String], output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, length: CryptomatorLengthPolicy, mut progress: F,
    ) -> Result<CryptomatorContentOutcome> {
        let source = self.resolve_file(components, &mut || {
            self.session.id()?;
            progress(0)
        });
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                output.abort().map_err(|_| CryptoError::CleanupFailed)?;
                return Err(error);
            }
        };
        self.session.decrypt_content(source, output, limits, length, progress)
    }

    /// Decrypt the target of a Cryptomator symlink (symlink.c9r) into memory.
    /// The link is not followed. Targets are bounded to 32 KiB of UTF-8.
    pub fn read_symlink<F: FnMut() -> Result<()>>(&self, components: &[String], mut checkpoint: F)
        -> Result<Zeroizing<String>>
    {
        if components.is_empty() || components.len() > 64 { return Err(CryptoError::InvalidInput); }
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        let (name, parents) = components.split_last().ok_or(CryptoError::InvalidInput)?;
        let (directory, path) = self.resolve_directory(parents, &mut check)?;
        let encoded = self.session.encrypt_name(&directory, name)?;
        let entry = join(&path, encoded.component())?;
        require_directory(self.storage.as_ref(), &entry)?;
        if let Some(expected) = encoded.name_mapping() {
            let mapping = read_small(self.storage.as_ref(), &join(&entry, "name.c9s")?, 368, &mut check)?;
            if mapping.as_slice() != expected.as_bytes() { return Err(CryptoError::Authentication); }
        }
        let (kind, _) = self.wrapper_kind(&entry, encoded.name_mapping().is_some(), &mut check)?;
        if kind != CryptomatorListedKind::UnsupportedLink { return Err(CryptoError::InvalidInput); }
        let source = ObservedRead::open(self.storage.as_ref(), &join(&entry, "symlink.c9r")?)?;
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = MemorySink { bytes: Zeroizing::new(Vec::new()), result: result.clone() };
        // The session holds its own lock while decrypting and checks
        // revocation itself: the callback must not call back into the session.
        self.session.decrypt_content(source, Box::new(sink), CryptomatorContentLimits { max_plaintext_bytes: 32 * 1024 },
            CryptomatorLengthPolicy::AllowUnauthenticated, |_| checkpoint())?;
        let bytes = result.lock().map_err(|_| CryptoError::Locked)?.take().ok_or(CryptoError::Io)?;
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| CryptoError::InvalidInput)?;
        Ok(Zeroizing::new(text))
    }

    fn resolve_file<F: FnMut() -> Result<()>>(&self, components: &[String], checkpoint: &mut F)
        -> Result<ObservedRead>
    {
        let file = self.resolve_file_path(components, checkpoint)?;
        ObservedRead::open(self.storage.as_ref(), &file)
    }
    /// Storage path of the content file (`contents.c9r` for shortened names).
    pub(crate) fn resolve_file_path<F: FnMut() -> Result<()>>(&self, components: &[String], checkpoint: &mut F)
        -> Result<ResourcePath>
    {
        if components.is_empty() || components.len() > 64 { return Err(CryptoError::InvalidInput); }
        let (name, parents) = components.split_last().ok_or(CryptoError::InvalidInput)?;
        let (directory, path) = self.resolve_directory(parents, checkpoint)?;
        checkpoint()?;
        let encoded = self.session.encrypt_name(&directory, name)?;
        let entry = join(&path, encoded.component())?;
        let file = if let Some(expected) = encoded.name_mapping() {
            require_directory(self.storage.as_ref(), &entry)?;
            let mapping = read_small(self.storage.as_ref(), &join(&entry, "name.c9s")?, 368, checkpoint)?;
            if mapping.as_slice() != expected.as_bytes() { return Err(CryptoError::Authentication); }
            join(&entry, "contents.c9r")?
        } else { entry };
        Ok(file)
    }

    pub(crate) fn data_path(&self, id: &CryptomatorDirectoryId) -> Result<ResourcePath> {
        let relative = self.session.directory_path(id)?;
        let mut path = self.root.clone();
        for component in relative.split('/') { path = join(&path, component)?; }
        Ok(path)
    }
}

fn join(parent: &ResourcePath, component: &str) -> Result<ResourcePath> {
    parent.join(component).map_err(|_| CryptoError::InvalidInput)
}
pub(crate) fn storage_error(error: paranoic_core::CoreError) -> CryptoError {
    use paranoic_core::ErrorCode;
    match error.code {
        ErrorCode::LimitExceeded => CryptoError::LimitExceeded,
        ErrorCode::NotFound => CryptoError::NotFound,
        ErrorCode::PermissionDenied => CryptoError::Permissions,
        ErrorCode::Unsupported => CryptoError::Unsupported,
        ErrorCode::Cancelled => CryptoError::Cancelled,
        _ => CryptoError::Io,
    }
}
fn require_directory(storage: &dyn Storage, path: &ResourcePath) -> Result<()> {
    let metadata = storage.metadata(path).map_err(|_| CryptoError::Io)?;
    if metadata.kind != EntryKind::Directory { return Err(CryptoError::InvalidInput); }
    Ok(())
}

// This detects observable changes, not an atomic filesystem snapshot or an
// authenticated total length. Final check happens before the codec sees EOF.
struct ObservedRead {
    handle: Box<dyn ReadHandle>,
    initial: Metadata,
    consumed: u64,
    ended: bool,
}
impl ObservedRead {
    fn open(storage: &dyn Storage, path: &ResourcePath) -> Result<Self> {
        let handle = storage.open_read(path).map_err(|_| CryptoError::Io)?;
        let initial = handle.metadata().map_err(|_| CryptoError::Io)?;
        if initial.kind != EntryKind::File { return Err(CryptoError::InvalidInput); }
        Ok(Self { handle, initial, consumed: 0, ended: false })
    }
}
impl Read for ObservedRead {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.ended { return Ok(0); }
        let count = self.handle.read(output)?;
        self.consumed = self.consumed.checked_add(count as u64)
            .ok_or_else(|| io::Error::other("source size overflow"))?;
        if self.consumed > self.initial.bytes { return Err(io::Error::other("source changed")); }
        if count == 0 {
            let after = self.handle.metadata().map_err(|_| io::Error::other("source metadata unavailable"))?;
            if self.consumed != self.initial.bytes || after != self.initial {
                return Err(io::Error::other("source changed"));
            }
            self.ended = true;
        }
        Ok(count)
    }
}

pub(crate) fn read_small<F: FnMut() -> Result<()>>(
    storage: &dyn Storage, path: &ResourcePath, limit: usize, checkpoint: &mut F,
) -> Result<Zeroizing<Vec<u8>>> {
    let mut input = ObservedRead::open(storage, path)?;
    if input.initial.bytes > limit as u64 { return Err(CryptoError::LimitExceeded); }
    let mut bytes = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0u8; 1024]);
    loop {
        checkpoint()?;
        let count = match input.read(&mut buffer[..]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if count == 0 { break; }
        if bytes.len() + count > limit { return Err(CryptoError::LimitExceeded); }
        bytes.extend_from_slice(&buffer[..count]);
    }
    checkpoint()?;
    Ok(bytes)
}


/// Bounded in-memory plaintext sink for tiny objects (symlink targets).
struct MemorySink {
    bytes: Zeroizing<Vec<u8>>,
    result: std::sync::Arc<std::sync::Mutex<Option<Zeroizing<Vec<u8>>>>>,
}
impl io::Write for MemorySink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > 32 * 1024 { return Err(io::Error::other("link target too long")); }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl StreamSink for MemorySink {
    fn commit(mut self: Box<Self>) -> Result<()> {
        let bytes = std::mem::take(&mut self.bytes);
        *self.result.lock().map_err(|_| CryptoError::Locked)? = Some(bytes);
        Ok(())
    }
    fn abort(self: Box<Self>) -> Result<()> { Ok(()) }
}
