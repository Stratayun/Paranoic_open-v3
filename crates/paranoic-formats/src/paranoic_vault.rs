//! Paranoic format v1 vault operations through Core (roadmap 1.8). Reads go
//! through Core grants; every change is a `VaultPlan`: ordered Core journal
//! operations prepared from a private spool and published create-new or
//! version-checked. Nothing is published directly by this module.
//!
//! Crash behaviour: each step is one journaled Core job. A plan interrupted
//! between steps leaves a consistent but possibly duplicated state (e.g. a
//! rename that stopped after the copy shows both names, never neither); the
//! journal keeps the failed step visible as NeedsAttention.
use std::io::{Read, Write};
use paranoic_core::{Core, CoreError, EntryKind as CoreKind, ErrorCode, JobId, JobRecord, JobState, Location, Metadata,
    Operation, PreparedTreeFile, ResourcePath, Storage};
use paranoic_crypto::{CryptoError, StreamSink};
use crate::paranoic_v1::{self as v1, Entry, EntryKind, KdfParams, NewFile, NewPlace, OsRandom, VaultKeys, ENTRY_HEADER_LEN};
use crate::paranoic_manifest::{self as man, HeaderHash, Manifest, RecordState, Verdict};

type Result<T> = paranoic_core::Result<T>;

/// Ordered Core operations; submit in order and stop at the first failure.
/// A plan that changes entries also carries the manifest changes it implies;
/// it must pass `ParanoicVault::with_manifest` before `run_plan` (which
/// refuses it otherwise), so no write can bypass the version manifest.
pub struct VaultPlan { pub steps: Vec<Operation>, intents: Vec<Intent>,
    /// Sequence of the manifest this plan publishes, if any. After the plan
    /// ran successfully, `ParanoicVault::confirm` pins it in the device anchor.
    publishes: Option<u64> }
impl VaultPlan {
    fn new(steps: Vec<Operation>, intents: Vec<Intent>) -> Self { Self { steps, intents, publishes: None } }
    /// True while manifest changes are still to be applied (`with_manifest`).
    pub fn needs_manifest(&self) -> bool { !self.intents.is_empty() }
}
/// One entry object a plan writes (`hash` = its new header) or deletes (None).
struct Intent { folder_path: String, object: String, hash: Option<HeaderHash> }
impl Intent { fn path(&self) -> String { format!("{}/{}", self.folder_path, self.object) } }

/// Spool path of the manifest prepared for a plan (next to the plan's spool).
pub fn manifest_spool_path(spool: &ResourcePath) -> Result<ResourcePath> { ResourcePath::parse(format!("{}-m", spool.as_str())) }

/// Ciphertext written to the spool, with the evidence Core checks on publication.
pub struct PreparedObject { spool: Location, target: Location, bytes: u64, blake3: [u8; 32],
    /// Storage folder path and object name of the target, and its header hash.
    folder_path: String, object: String, header: Option<HeaderHash> }
impl PreparedObject {
    pub fn spool(&self) -> &Location { &self.spool }
    pub fn target(&self) -> &Location { &self.target }
}

/// Password and parameters of a vault being created.
#[derive(Clone, Copy)]
pub struct VaultSettings<'a> { pub password: &'a [u8], pub kdf: KdfParams, pub padding: bool }

/// Private spool where ciphertext is prepared before Core publishes it.
#[derive(Clone, Copy)]
pub struct Spool<'a> { pub storage: &'a dyn Storage, pub path: &'a ResourcePath }
/// An existing entry: its folder, name and kind.
#[derive(Clone, Copy)]
pub struct EntryRef<'a> { pub folder: &'a [u8; 16], pub name: &'a str, pub kind: EntryKind }

/// A listed entry. Damaged or foreign objects are counted, not shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultEntry { pub name: String, pub folder: bool, pub size: u64, pub modified_ms: u64 }
#[derive(Debug, Default)]
pub struct VaultListing { pub entries: Vec<VaultEntry>, pub damaged: usize }

/// Per-device memory of the highest manifest sequence seen for each vault
/// (spec §9). Kept outside the vault and never synced: it is what detects a
/// whole vault returned to an older state on a device that saw a newer one.
pub trait ManifestAnchor: Send + Sync {
    fn get(&self, vault_id: &[u8; 16]) -> Result<Option<u64>>;
    /// Remember `sequence` if it is higher than the stored one.
    fn raise(&self, vault_id: &[u8; 16], sequence: u64) -> Result<()>;
}

/// Open vault: location of its root folder and the unlocked keys.
pub struct ParanoicVault { root: Location, keys: VaultKeys, anchor: Option<std::sync::Arc<dyn ManifestAnchor>> }

impl ParanoicVault {
    /// Plan creating a new vault folder at `target` (must not exist).
    pub fn plan_create(target: Location, password: &[u8], kdf: KdfParams, padding: bool,
        checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<(VaultPlan, ParanoicVault)> {
        let (header, mut keys) = v1::create_vault(password, kdf, padding, &mut OsRandom, checkpoint).map_err(crypto)?;
        // New vaults always carry the version manifest (spec §9, owner decision Р-2).
        let header = v1::require_manifest(&header, &keys).map_err(crypto)?;
        keys.manifest_required = true;
        let manifest = man::seal(&keys, &Manifest::default(), &mut OsRandom).map_err(crypto)?;
        let vault = Self::from_keys(target.clone(), keys)?;
        let mut steps = vec![Operation::PublishTree { target: target.clone(), files: vec![
            PreparedTreeFile { path: ResourcePath::parse(man::MANIFEST_FILE)?, bytes: manifest },
            PreparedTreeFile { path: ResourcePath::parse(v1::VAULT_FILE)?, bytes: header }] }];
        steps.extend(vault.folder_dirs(&vault.keys.vault_id)?.into_iter().map(|target| Operation::CreateDirectory { target }));
        Ok((VaultPlan::new(steps, Vec::new()), vault))
    }

    pub fn open(core: &Core, root: Location, password: &[u8], checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<Self> {
        let header = read_all(core, &child(&root, v1::VAULT_FILE)?, 4096)?;
        let keys = v1::unlock(&header, password, checkpoint).map_err(unlock_error)?;
        Self::from_keys(root, keys)
    }
    pub fn open_recovery(core: &Core, root: Location, recovery: &[u8; 32]) -> Result<Self> {
        let header = read_all(core, &child(&root, v1::VAULT_FILE)?, 4096)?;
        Self::from_keys(root, v1::unlock_recovery(&header, recovery).map_err(unlock_error)?)
    }
    /// The version manifest is mandatory (spec §9, owner decision 2026-11-13):
    /// no release ever created a vault without it, so one is only a test build
    /// leftover and is refused instead of being read without protection.
    fn from_keys(root: Location, keys: VaultKeys) -> Result<Self> {
        if !keys.manifest_required {
            return Err(CoreError::new(ErrorCode::Unsupported, "encrypted folder from a test build without the version manifest; create it again"));
        }
        Ok(Self { root, keys, anchor: None })
    }
    /// Check every manifest read against this device's anchor (spec §9).
    pub fn with_anchor(mut self, anchor: std::sync::Arc<dyn ManifestAnchor>) -> Self { self.anchor = Some(anchor); self }

    pub fn root_folder(&self) -> [u8; 16] { self.keys.vault_id }
    pub fn keys(&self) -> &VaultKeys { &self.keys }

    /// Folder ID of a plaintext path (components below the vault root).
    pub fn resolve(&self, core: &Core, path: &[String]) -> Result<[u8; 16]> {
        let mut folder = self.keys.vault_id;
        let manifest = self.load_manifest(core)?;
        for name in path {
            let (_, entry) = self.entry_with(core, manifest.as_ref(), &folder, name, EntryKind::Folder)?;
            match entry { Entry::Folder { folder_id, .. } => folder = folder_id, Entry::File { .. } => return Err(invalid()) }
        }
        Ok(folder)
    }

    /// Complete listing of one folder; unreadable objects are counted as damaged.
    pub fn list(&self, core: &Core, folder: &[u8; 16]) -> Result<VaultListing> {
        let location = self.folder_location(folder)?;
        let manifest = self.load_manifest(core)?;
        let mut listing = VaultListing::default();
        for item in core.list(&location)? {
            let (_, object) = item.location.path.parent_and_name()?;
            if !(object.len() == 28 && (object.ends_with(".f") || object.ends_with(".d"))) || item.metadata.kind != CoreKind::File {
                listing.damaged += 1; continue;
            }
            let header = match read_header(core, &item.location) { Ok(h) => h, Err(_) => { listing.damaged += 1; continue } };
            // Not the version the manifest records (older, deleted, foreign): not listed.
            if self.verify(manifest.as_ref(), folder, object, &header).is_err() { listing.damaged += 1; continue; }
            match self.keys.open_entry(folder, object, &header) {
                Ok(Entry::File { name, modified_ms, size }) => listing.entries.push(VaultEntry { name, folder: false, size, modified_ms }),
                Ok(Entry::Folder { name, modified_ms, .. }) => listing.entries.push(VaultEntry { name, folder: true, size: 0, modified_ms }),
                Err(_) => listing.damaged += 1,
            }
        }
        listing.entries.sort_by(|a, b| (!a.folder, &a.name).cmp(&(!b.folder, &b.name)));
        Ok(listing)
    }

    /// Encrypt a plaintext file into the spool for adding/replacing `name` in `folder`.
    pub fn prepare_file(&self, folder: &[u8; 16], file: NewFile<'_>, input: impl Read, spool: Spool<'_>,
        checkpoint: &mut dyn FnMut(u64) -> paranoic_crypto::Result<()>) -> Result<PreparedObject> {
        let target = self.object_location(folder, file.name, EntryKind::File)?;
        let names = self.names(folder, file.name, EntryKind::File)?;
        self.spool(spool, target, names, |out| self.keys.encrypt_file(folder, file, input, out, &mut OsRandom, checkpoint))
    }
    /// Add a new file (fails if the name exists).
    pub fn plan_add(&self, prepared: &PreparedObject) -> VaultPlan {
        VaultPlan::new(vec![Operation::CopyExpected { source: prepared.spool.clone(), target: prepared.target.clone(),
            bytes: prepared.bytes, blake3: prepared.blake3 }], vec![prepared.intent()])
    }
    /// Replace an existing file only if it is still exactly `expected` (current metadata from `stat`).
    pub fn plan_replace(&self, prepared: &PreparedObject, expected: Metadata) -> VaultPlan {
        VaultPlan::new(vec![Operation::ReplaceExpected { source: prepared.spool.clone(), target: prepared.target.clone(),
            bytes: prepared.bytes, blake3: prepared.blake3, expected_target: expected }], vec![prepared.intent()])
    }
    /// Replace only the exact encrypted object `expected_blake3` (BLAKE3 of
    /// its ciphertext, from `object_version`): Core rechecks it when the job
    /// runs, so another write with the same size/time is never overwritten (У-70).
    pub fn plan_replace_content(&self, prepared: &PreparedObject, expected: Metadata, expected_blake3: [u8; 32]) -> VaultPlan {
        VaultPlan::new(vec![Operation::ReplaceContentExpected { source: prepared.spool.clone(), target: prepared.target.clone(),
            bytes: prepared.bytes, blake3: prepared.blake3, expected_target: expected, expected_target_blake3: expected_blake3 }],
            vec![prepared.intent()])
    }
    /// Strong version of a file: storage metadata and BLAKE3 of the encrypted
    /// object (every write has a fresh header, so equal hashes mean the same write).
    pub fn object_version(&self, core: &Core, folder: &[u8; 16], name: &str) -> Result<(Metadata, [u8; 32])> {
        let location = self.object_location(folder, name, EntryKind::File)?;
        let mut input = core.open_read(&location)?;
        let before = input.metadata()?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 16];
        loop {
            let n = input.read(&mut buffer).map_err(|_| CoreError::new(ErrorCode::Io, "vault file unreadable"))?;
            if n == 0 { break; }
            hasher.update(&buffer[..n]);
        }
        if input.metadata()? != before || core.metadata(&location)? != before {
            return Err(CoreError::new(ErrorCode::SourceChanged, "vault file changed while reading"));
        }
        Ok((before, *hasher.finalize().as_bytes()))
    }
    /// Storage metadata of an entry object (for replace/delete expectations).
    pub fn stat(&self, core: &Core, folder: &[u8; 16], name: &str, kind: EntryKind) -> Result<Metadata> {
        core.metadata(&self.object_location(folder, name, kind)?)
    }

    /// Decrypt a file; plaintext reaches `output` only after full verification.
    pub fn read_file(&self, core: &Core, folder: &[u8; 16], name: &str, output: Box<dyn StreamSink>, max_size: u64,
        checkpoint: &mut dyn FnMut(u64) -> paranoic_crypto::Result<()>) -> Result<Entry> {
        // Cryptographic verification is only one part of publication: keep the
        // real sink staged until both the handle and its current path are stable.
        let pending = std::rc::Rc::new(std::cell::RefCell::new(Some(output)));
        let result = (|| {
            let manifest = self.load_manifest(core)?;
            let location = self.object_location(folder, name, EntryKind::File)?;
            let object = self.keys.entry_name(folder, name, EntryKind::File).map_err(crypto)?;
            let mut input = core.open_read(&location)?;
            let before = input.metadata()?;
            // The header bytes actually decrypted are the ones checked against the manifest.
            let mut tap = HeaderTap { inner: &mut input, head: Vec::with_capacity(ENTRY_HEADER_LEN) };
            let entry = self.keys.decrypt_file(folder, &object, &mut tap,
                Box::new(DeferredCommit(pending.clone())), max_size, checkpoint).map_err(crypto)?;
            let head = std::mem::take(&mut tap.head);
            self.verify(manifest.as_ref(), folder, &object, &head)?;
            if input.metadata()? != before || core.metadata(&location)? != before {
                return Err(CoreError::new(ErrorCode::SourceChanged, "vault file changed while reading"));
            }
            Ok(entry)
        })();
        match result {
            Ok(entry) => {
                pending.borrow_mut().take().ok_or_else(invalid)?.commit().map_err(crypto)?;
                Ok(entry)
            }
            Err(error) => {
                if let Some(sink) = pending.borrow_mut().take() { let _ = sink.abort(); }
                Err(error)
            }
        }
    }

    /// Plan 4.7: a file opened for random-access reading in memory (viewers,
    /// archives inside the vault). The header is checked against the
    /// manifest and the object's length against the header; every read
    /// checks that the object is still the version opened.
    pub fn open_reader(&self, core: &Core, folder: &[u8; 16], name: &str) -> Result<VaultFileReader> {
        let manifest = self.load_manifest(core)?;
        let location = self.object_location(folder, name, EntryKind::File)?;
        let object = self.keys.entry_name(folder, name, EntryKind::File).map_err(crypto)?;
        let mut handle = core.open_read(&location)?;
        let before = handle.metadata()?;
        let mut header = vec![0u8; ENTRY_HEADER_LEN];
        handle.read_exact(&mut header).map_err(|_| CoreError::new(ErrorCode::Integrity, "vault file header unreadable"))?;
        self.verify(manifest.as_ref(), folder, &object, &header)?;
        let (_, cipher) = self.keys.file_cipher(folder, &object, &header).map_err(crypto)?;
        if before.bytes != cipher.object_len() {
            return Err(CoreError::new(ErrorCode::Integrity, "vault file length does not match its header"));
        }
        Ok(VaultFileReader { handle, before, cipher })
    }

    /// New folder: its storage directories first, then the entry in the parent.
    pub fn prepare_folder(&self, core: &Core, parent: &[u8; 16], name: &str, modified_ms: u64, spool: Spool<'_>)
        -> Result<(PreparedObject, VaultPlan)> {
        let mut child = [0u8; 16];
        v1::RandomSource::fill(&mut OsRandom, &mut child).map_err(crypto)?;
        let target = self.object_location(parent, name, EntryKind::Folder)?;
        let names = self.names(parent, name, EntryKind::Folder)?;
        let prepared = self.spool(spool, target, names, |out| {
            let header = self.keys.seal_folder(parent, name, modified_ms, &child, &mut OsRandom)?;
            out.write_all(&header).map_err(|_| CryptoError::Io)
        })?;
        let mut steps = Vec::new();
        for dir in self.folder_dirs(&child)? {
            match core.metadata(&dir) {
                Ok(m) if m.kind == CoreKind::Directory => {}
                Ok(_) => return Err(invalid()),
                Err(e) if e.code == ErrorCode::NotFound => steps.push(Operation::CreateDirectory { target: dir }),
                Err(e) => return Err(e),
            }
        }
        steps.push(Operation::CopyExpected { source: prepared.spool.clone(), target: prepared.target.clone(),
            bytes: prepared.bytes, blake3: prepared.blake3 });
        let intents = vec![prepared.intent()];
        Ok((prepared, VaultPlan::new(steps, intents)))
    }

    /// Delete a file, only the version observed now.
    pub fn plan_delete_file(&self, core: &Core, folder: &[u8; 16], name: &str) -> Result<VaultPlan> {
        let target = self.object_location(folder, name, EntryKind::File)?;
        let expected = core.metadata(&target)?;
        let (folder_path, object) = self.names(folder, name, EntryKind::File)?;
        Ok(VaultPlan::new(vec![Operation::DeleteFileExpected { target, expected, expected_revision: None }], vec![Intent { folder_path, object, hash: None }]))
    }
    /// Delete the storage directory first, atomically refusing a concurrent add.
    /// Only then remove its entry. A failed step never orphans newly added files.
    pub fn plan_delete_folder(&self, core: &Core, parent: &[u8; 16], name: &str) -> Result<VaultPlan> {
        let (location, entry) = self.entry(core, parent, name, EntryKind::Folder)?;
        let Entry::Folder { folder_id, .. } = entry else { return Err(invalid()) };
        let storage = self.folder_location(&folder_id)?;
        match core.list(&storage) {
            Ok(items) if !items.is_empty() => return Err(CoreError::new(ErrorCode::AlreadyExists, "folder is not empty")),
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
        let expected = core.metadata(&location)?;
        let mut steps = Vec::new();
        match core.metadata(&storage) {
            Ok(_) => steps.push(Operation::DeleteEmptyDirectory { target: storage }),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(error),
        }
        steps.push(Operation::DeleteFileExpected { target: location, expected, expected_revision: None });
        let (folder_path, object) = self.names(parent, name, EntryKind::Folder)?;
        Ok(VaultPlan::new(steps, vec![Intent { folder_path, object, hash: None }]))
    }

    /// Rename and/or move an entry: a new header plus the unchanged ciphertext
    /// is prepared, published create-new under the new name, then the old
    /// object is deleted if unchanged. Folders keep their folder ID (contents
    /// are not touched).
    pub fn prepare_rename(&self, core: &Core, from: EntryRef<'_>, to: NewPlace<'_>, spool: Spool<'_>,
        checkpoint: &mut dyn FnMut(u64) -> paranoic_crypto::Result<()>) -> Result<VaultPlan> {
        let EntryRef { folder, name, kind } = from;
        if kind == EntryKind::Folder && folder != to.folder {
            let Entry::Folder { folder_id, .. } = self.entry(core, folder, name, kind)?.1 else { return Err(invalid()) };
            // Flat encrypted storage does not have the filesystem's built-in
            // ancestor check: moving an entry into its subtree would hide it.
            let mut pending = vec![(folder_id, 0usize)];
            let mut seen = std::collections::HashSet::new();
            while let Some((child, depth)) = pending.pop() {
                checkpoint(0).map_err(crypto)?;
                if &child == to.folder { return Err(CoreError::new(ErrorCode::InvalidInput, "cannot move a folder into its subtree")); }
                if depth > 64 || seen.len() >= 100_000 { return Err(CoreError::new(ErrorCode::LimitExceeded, "vault tree too large")); }
                if !seen.insert(child) { return Err(CoreError::new(ErrorCode::Integrity, "vault folder cycle or alias")); }
                let listing = self.list(core, &child)?;
                if listing.damaged != 0 { return Err(CoreError::new(ErrorCode::Integrity, "cannot move a damaged folder tree")); }
                for entry in listing.entries.into_iter().filter(|e| e.folder) {
                    pending.push((self.resolve_child(core, &child, &entry.name)?, depth + 1));
                }
            }
        }
        let source = self.object_location(folder, name, kind)?;
        let object = self.keys.entry_name(folder, name, kind).map_err(crypto)?;
        let expected = core.metadata(&source)?;
        // The source header must be the version the manifest records, as for
        // reading: an older authentic version must not get a new name and a
        // new current record (audit of 1.8, finding 1).
        let manifest = self.load_manifest(core)?;
        let verified = read_header(core, &source)?;
        self.verify(manifest.as_ref(), folder, &object, &verified)?;
        let target = self.object_location(to.folder, to.name, kind)?;
        let names = self.names(to.folder, to.name, kind)?;
        let mut input = core.open_read(&source)?;
        if input.metadata()? != expected { return Err(CoreError::new(ErrorCode::SourceChanged, "entry changed")); }
        let prepared = self.spool(spool, target, names, |out| {
            let mut header = vec![0u8; ENTRY_HEADER_LEN];
            input.read_exact(&mut header).map_err(|_| CryptoError::Authentication)?;
            // Exactly the header that was checked against the manifest.
            if header != verified { return Err(CryptoError::Authentication); }
            out.write_all(&self.keys.reissue_header(folder, &object, &header, &to, &mut OsRandom)?)
                .map_err(|_| CryptoError::Io)?;
            // Chunks are bound to the file, not to its name: copied as they are.
            let mut buffer = vec![0u8; 65_536]; let mut copied = 0u64;
            loop {
                checkpoint(copied)?;
                let n = input.read(&mut buffer).map_err(|_| CryptoError::Io)?;
                if n == 0 { break; }
                out.write_all(&buffer[..n]).map_err(|_| CryptoError::Io)?; copied += n as u64;
            }
            if ENTRY_HEADER_LEN as u64 + copied != expected.bytes { return Err(CryptoError::Authentication); }
            Ok(())
        })?;
        let (folder_path, object_name) = self.names(folder, name, kind)?;
        Ok(VaultPlan::new(vec![
            Operation::CopyExpected { source: prepared.spool.clone(), target: prepared.target.clone(), bytes: prepared.bytes, blake3: prepared.blake3 },
            Operation::DeleteFileExpected { target: source, expected, expected_revision: None },
        ], vec![prepared.intent(), Intent { folder_path, object: object_name, hash: None }]))
    }

    /// New password (spec §3). `old_password` must open the stored header of
    /// THIS vault; the header is then replaced only if it is still the version
    /// read. Master key and contents stay; copies of the old header elsewhere
    /// (cloud versions, backups) still open with the old password: only a
    /// master-key rotation (`rotate_into`) revokes those.
    pub fn plan_change_password(&self, core: &Core, old_password: &[u8], new_password: &[u8], spool: Spool<'_>,
        checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<VaultPlan> {
        let (bytes, expected) = self.verified_header(core, old_password, checkpoint)?;
        let header = v1::change_password(&bytes, &self.keys, new_password, &mut OsRandom, checkpoint).map_err(crypto)?;
        self.plan_header(&header, expected, spool)
    }
    /// Forgotten password (R-8): the recovery key opens THIS vault's stored
    /// header, which then gets a new password slot. Replaced only if still the
    /// version read. The recovery slot itself is kept.
    pub fn plan_reset_password(&self, core: &Core, recovery: &[u8; 32], new_password: &[u8], spool: Spool<'_>,
        checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<VaultPlan> {
        let (bytes, expected) = self.verified_header_with(core, &mut |bytes| v1::unlock_recovery(bytes, recovery))?;
        let header = v1::change_password(&bytes, &self.keys, new_password, &mut OsRandom, checkpoint).map_err(crypto)?;
        self.plan_header(&header, expected, spool)
    }
    /// Add or replace the recovery slot after checking `password`. Returns the
    /// plan and the recovery key text, to be shown once after the plan ran.
    pub fn plan_set_recovery(&self, core: &Core, password: &[u8], spool: Spool<'_>,
        checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<(VaultPlan, zeroize::Zeroizing<String>)> {
        let (bytes, expected) = self.verified_header(core, password, checkpoint)?;
        let (header, recovery) = v1::set_recovery(&bytes, &self.keys, &mut OsRandom).map_err(crypto)?;
        Ok((self.plan_header(&header, expected, spool)?, v1::recovery_text(&recovery)))
    }
    /// Current header bytes and their storage version, after `password`
    /// opened them as this vault. Read twice-checked: a header replaced while
    /// reading is reported, not used.
    fn verified_header(&self, core: &Core, password: &[u8],
        checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<(Vec<u8>, Metadata)> {
        self.verified_header_with(core, &mut |bytes| v1::unlock(bytes, password, checkpoint))
    }
    fn verified_header_with(&self, core: &Core, unlock: &mut dyn FnMut(&[u8]) -> paranoic_crypto::Result<VaultKeys>)
        -> Result<(Vec<u8>, Metadata)> {
        let location = child(&self.root, v1::VAULT_FILE)?;
        let expected = core.metadata(&location)?;
        let bytes = read_all(core, &location, 4096)?;
        if core.metadata(&location)? != expected { return Err(CoreError::new(ErrorCode::SourceChanged, "vault header changed")); }
        let keys = unlock(&bytes).map_err(unlock_error)?;
        if keys.vault_id != self.keys.vault_id { return Err(CoreError::new(ErrorCode::PermissionDenied, "header of another vault")); }
        Ok((bytes, expected))
    }
    /// Version-checked replacement of the vault header. Entries and the
    /// manifest are untouched, so the plan carries no manifest intents.
    fn plan_header(&self, header: &[u8], expected: Metadata, spool: Spool<'_>) -> Result<VaultPlan> {
        let Spool { storage, path } = spool;
        let mut staged = storage.stage(path, JobId(uuid::Uuid::new_v4()))?;
        if let Err(error) = staged.write_all(header) { let _ = staged.abort(); return Err(CoreError::io(error)); }
        staged.publish()?;
        Ok(VaultPlan::new(vec![Operation::ReplaceExpected { source: Location { storage: storage.id(), path: path.clone() },
            target: child(&self.root, v1::VAULT_FILE)?, bytes: header.len() as u64, blake3: *blake3::hash(header).as_bytes(),
            expected_target: expected }], Vec::new()))
    }

    /// Master-key rotation (spec §3): re-encrypt the whole tree into a NEW vault
    /// at `target` with fresh keys and the given password. Each file is
    /// decrypted, verified chunk by chunk and re-encrypted on the fly into the
    /// spool (no plaintext on disk), then published create-new. The old vault
    /// is never modified; delete it only after checking the new one. A new
    /// recovery key, if wanted, is set on the new vault separately.
    pub fn rotate_into(&self, core: &Core, target: Location, settings: VaultSettings<'_>, spool: &dyn Storage,
        timeout: std::time::Duration, checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<ParanoicVault> {
        // Nothing is created unless every recorded object is there: a copy
        // silently missing a file must not look complete (audit of 1.8, finding 3).
        self.verify_complete(core, checkpoint)?;
        let (plan, fresh) = Self::plan_create(target, settings.password, settings.kdf, settings.padding, checkpoint)?;
        expect_success(run_plan(core, &plan, timeout)?, plan.steps.len())?;
        let mut stack = vec![(self.keys.vault_id, fresh.keys.vault_id, 0usize)];
        while let Some((old_folder, new_folder, depth)) = stack.pop() {
            if depth > 64 { return Err(CoreError::new(ErrorCode::LimitExceeded, "vault tree too deep")); }
            checkpoint().map_err(crypto)?;
            let listing = self.list(core, &old_folder)?;
            if listing.damaged != 0 {
                return Err(CoreError::new(ErrorCode::Integrity, "vault has damaged objects; rotation stopped, old vault unchanged"));
            }
            for entry in listing.entries {
                let path = ResourcePath::parse(format!("rotate-{}", uuid::Uuid::new_v4().simple()))?;
                if entry.folder {
                    let (_, found) = self.entry(core, &old_folder, &entry.name, EntryKind::Folder)?;
                    let Entry::Folder { folder_id, .. } = found else { return Err(invalid()) };
                    let (_, plan) = fresh.prepare_folder(core, &new_folder, &entry.name, entry.modified_ms, Spool { storage: spool, path: &path })?;
                    let done = fresh.run_with_manifest(core, plan, Spool { storage: spool, path: &path }, timeout);
                    let _ = spool.delete_file(&path);
                    let _ = spool.delete_file(&manifest_spool_path(&path)?);
                    done?;
                    let new_child = fresh.resolve_child(core, &new_folder, &entry.name)?;
                    stack.push((folder_id, new_child, depth + 1));
                } else {
                    let prepared = self.reencrypt(core, &old_folder, &entry, &fresh, &new_folder, spool, &path)?;
                    let done = fresh.run_with_manifest(core, fresh.plan_add(&prepared), Spool { storage: spool, path: &path }, timeout);
                    let _ = spool.delete_file(&path);
                    let _ = spool.delete_file(&manifest_spool_path(&path)?);
                    done?;
                }
            }
        }
        Ok(fresh)
    }
    /// Every object the manifest records as present must be found by walking
    /// the tree from the root, and nothing may be damaged or in flight. A
    /// listing alone cannot see an object that vanished from the storage;
    /// whole-vault operations (rotation) check this first.
    pub fn verify_complete(&self, core: &Core, checkpoint: &mut dyn FnMut() -> paranoic_crypto::Result<()>) -> Result<()> {
        let manifest = self.load_manifest(core)?.ok_or_else(|| CoreError::new(ErrorCode::Integrity, "vault manifest missing"))?;
        if manifest.has_pending() {
            return Err(CoreError::new(ErrorCode::Busy, "vault has an unfinished change; make any change or reopen it to finish it first"));
        }
        let mut seen: std::collections::HashSet<man::Locator> = std::collections::HashSet::new();
        let mut folders = std::collections::HashSet::new();
        let mut stack = vec![(self.keys.vault_id, 0usize)];
        while let Some((folder, depth)) = stack.pop() {
            checkpoint().map_err(crypto)?;
            if depth > 64 || folders.len() >= 100_000 { return Err(CoreError::new(ErrorCode::LimitExceeded, "vault tree too large")); }
            if !folders.insert(folder) { return Err(CoreError::new(ErrorCode::Integrity, "vault folder cycle or alias")); }
            let listing = self.list(core, &folder)?;
            if listing.damaged != 0 { return Err(CoreError::new(ErrorCode::Integrity, "vault has damaged objects")); }
            let folder_path = self.keys.folder_path(&folder).map_err(crypto)?;
            for entry in listing.entries {
                let kind = if entry.folder { EntryKind::Folder } else { EntryKind::File };
                let object = self.keys.entry_name(&folder, &entry.name, kind).map_err(crypto)?;
                seen.insert(man::locator(&self.keys, &folder_path, &object).map_err(crypto)?);
                if entry.folder { stack.push((self.resolve_child(core, &folder, &entry.name)?, depth + 1)); }
            }
        }
        let missing = manifest.records().filter(|(locator, record)| record.state == RecordState::Present && !seen.contains(*locator)).count();
        if missing != 0 {
            return Err(CoreError::new(ErrorCode::Integrity, format!("{missing} object(s) recorded in the vault manifest are missing from the storage")));
        }
        Ok(())
    }
    /// `with_manifest`, run every step, then settle (spec §9). Errors if any step
    /// failed. The caller removes `spool.path` and its manifest spool.
    pub fn run_with_manifest(&self, core: &Core, plan: VaultPlan, spool: Spool<'_>, timeout: std::time::Duration) -> Result<()> {
        let plan = self.with_manifest(core, plan, spool)?;
        expect_success(run_plan(core, &plan, timeout)?, plan.steps.len())?;
        self.confirm(&plan)?;
        if let Some(settle) = self.plan_settle(core, spool)? {
            expect_success(run_plan(core, &settle, timeout)?, settle.steps.len())?;
            self.confirm(&settle)?;
        }
        Ok(())
    }
    fn resolve_child(&self, core: &Core, folder: &[u8; 16], name: &str) -> Result<[u8; 16]> {
        match self.entry(core, folder, name, EntryKind::Folder)?.1 { Entry::Folder { folder_id, .. } => Ok(folder_id), Entry::File { .. } => Err(invalid()) }
    }
    #[allow(clippy::too_many_arguments)]
    fn reencrypt(&self, core: &Core, old_folder: &[u8; 16], entry: &VaultEntry, fresh: &ParanoicVault, new_folder: &[u8; 16],
        spool: &dyn Storage, path: &ResourcePath) -> Result<PreparedObject> {
        let target = fresh.object_location(new_folder, &entry.name, EntryKind::File)?;
        let staged = spool.stage(path, JobId(uuid::Uuid::new_v4()))?;
        let shared = std::rc::Rc::new(std::cell::RefCell::new(Shared { staged: Some(staged), hash: blake3::Hasher::new(), bytes: 0, failed: false, head: Vec::new() }));
        let writer = fresh.keys.file_writer(new_folder, NewFile { name: &entry.name, modified_ms: entry.modified_ms, size: entry.size },
            Box::new(SharedWrite(shared.clone())), &mut OsRandom).map_err(crypto)?;
        let sink = Reencrypt { writer: Some(writer), shared: shared.clone() };
        let result = self.read_file(core, old_folder, &entry.name, Box::new(sink), u64::MAX, &mut |_| Ok(()));
        let mut state = shared.borrow_mut();
        match result {
            Ok(_) if !state.failed && state.staged.is_none() => {
                let (folder_path, object) = fresh.names(new_folder, &entry.name, EntryKind::File)?;
                let header = if state.head.len() == ENTRY_HEADER_LEN { Some(man::header_hash(&state.head).map_err(crypto)?) } else { None };
                Ok(PreparedObject { spool: Location { storage: spool.id(), path: path.clone() }, target, bytes: state.bytes,
                    blake3: *state.hash.finalize().as_bytes(), folder_path, object, header })
            }
            Ok(_) | Err(_) => {
                if let Some(staged) = state.staged.take() { let _ = staged.abort(); } else { let _ = spool.delete_file(path); }
                Err(result.err().unwrap_or_else(|| CoreError::new(ErrorCode::Io, "re-encryption failed")))
            }
        }
    }

    fn entry(&self, core: &Core, folder: &[u8; 16], name: &str, kind: EntryKind) -> Result<(Location, Entry)> {
        let manifest = self.load_manifest(core)?;
        self.entry_with(core, manifest.as_ref(), folder, name, kind)
    }
    fn entry_with(&self, core: &Core, manifest: Option<&Manifest>, folder: &[u8; 16], name: &str, kind: EntryKind) -> Result<(Location, Entry)> {
        let location = self.object_location(folder, name, kind)?;
        let object = self.keys.entry_name(folder, name, kind).map_err(crypto)?;
        let header = read_header(core, &location)?;
        self.verify(manifest, folder, &object, &header)?;
        Ok((location, self.keys.open_entry(folder, &object, &header).map_err(crypto)?))
    }
    /// Storage folder path and object name of an entry.
    fn names(&self, folder: &[u8; 16], name: &str, kind: EntryKind) -> Result<(String, String)> {
        Ok((self.keys.folder_path(folder).map_err(crypto)?, self.keys.entry_name(folder, name, kind).map_err(crypto)?))
    }

    /// The current manifest, checked against this device's anchor.
    fn load_manifest(&self, core: &Core) -> Result<Option<Manifest>> {
        let bytes = read_all(core, &child(&self.root, man::MANIFEST_FILE)?, man::MAX_SEALED)?;
        self.open_manifest(&bytes)
    }
    fn open_manifest(&self, bytes: &[u8]) -> Result<Option<Manifest>> {
        let manifest = man::open(&self.keys, bytes)
            .map_err(|_| CoreError::new(ErrorCode::Integrity, "vault manifest failed authentication"))?;
        if let Some(anchor) = &self.anchor {
            if anchor.get(&self.keys.vault_id)?.is_some_and(|seen| manifest.sequence < seen) {
                return Err(CoreError::new(ErrorCode::Integrity, "vault was returned to an older state (manifest older than this device has seen)"));
            }
            anchor.raise(&self.keys.vault_id, manifest.sequence)?;
        }
        Ok(Some(manifest))
    }
    /// The header found at `folder`/`object` must be the version the manifest records.
    fn verify(&self, manifest: Option<&Manifest>, folder: &[u8; 16], object: &str, header: &[u8]) -> Result<()> {
        let Some(manifest) = manifest else { return Ok(()) };
        let locator = man::locator(&self.keys, &self.keys.folder_path(folder).map_err(crypto)?, object).map_err(crypto)?;
        match manifest.check(&locator, &man::header_hash(header).map_err(crypto)?) {
            Verdict::Accepted => Ok(()),
            Verdict::Unknown | Verdict::Mismatch => Err(CoreError::new(ErrorCode::Integrity,
                "vault entry is not the version recorded in the manifest (rollback or tampering)")),
        }
    }
    /// Hash of the header stored at `path` (relative to the root), None if absent.
    fn observe(&self, core: &Core, path: &str) -> paranoic_crypto::Result<Option<HeaderHash>> {
        let location = child(&self.root, path).map_err(|_| CryptoError::InvalidInput)?;
        match read_header(core, &location) {
            Ok(header) => man::header_hash(&header).map(Some),
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            // Unreadable: a hash nothing matches, so the record stays unsettled.
            Err(error) if error.code == ErrorCode::Integrity => Ok(Some([0xff; 16])),
            Err(_) => Err(CryptoError::Io),
        }
    }

    /// Prepare `plan` for running on a vault with the manifest: read the current
    /// manifest, settle changes left in flight, record this plan's changes, raise
    /// the sequence and put a version-checked manifest replacement FIRST (spec
    /// §9). A concurrent writer makes that step fail as changed: re-read and plan
    /// again. After the plan succeeded, run `plan_settle` so the old versions
    /// stop being acceptable. The manifest is staged at
    /// `manifest_spool_path(spool.path)`; remove it after running.
    pub fn with_manifest(&self, core: &Core, mut plan: VaultPlan, spool: Spool<'_>) -> Result<VaultPlan> {
        if plan.intents.is_empty() { return Ok(plan); }
        let intents = std::mem::take(&mut plan.intents);
        let (step, sequence) = self.manifest_step(core, spool, &intents)?
            .ok_or_else(|| CoreError::new(ErrorCode::Internal, "manifest step missing"))?;
        plan.steps.insert(0, step);
        plan.publishes = Some(sequence);
        Ok(plan)
    }
    /// Call after `plan` ran successfully: pins the manifest version it
    /// published in this device's anchor at once (spec §9), so an older
    /// authentic manifest is refused even before the next read. Before this,
    /// the anchor could lag one write behind (audit of 1.8, finding 2).
    pub fn confirm(&self, plan: &VaultPlan) -> Result<()> {
        match (&self.anchor, plan.publishes) {
            (Some(anchor), Some(sequence)) => anchor.raise(&self.keys.vault_id, sequence),
            _ => Ok(()),
        }
    }
    /// After a plan of `with_manifest` succeeded: one manifest replacement that
    /// settles its records, so an older version or a deleted object is refused
    /// again at once (spec §9). None when nothing is in flight. If it does not
    /// run (crash), the next change settles the records.
    pub fn plan_settle(&self, core: &Core, spool: Spool<'_>) -> Result<Option<VaultPlan>> {
        Ok(self.manifest_step(core, spool, &[])?.map(|(step, sequence)| {
            let mut plan = VaultPlan::new(vec![step], Vec::new());
            plan.publishes = Some(sequence);
            plan
        }))
    }
    fn manifest_step(&self, core: &Core, spool: Spool<'_>, intents: &[Intent]) -> Result<Option<(Operation, u64)>> {
        let location = child(&self.root, man::MANIFEST_FILE)?;
        let expected = core.metadata(&location)?;
        // Audit A27: the replacement is bound to the exact bytes this change was derived from
        // (BLAKE3), not only to size and time, so a concurrent writer's manifest is never lost.
        let current = read_all(core, &location, man::MAX_SEALED)?;
        let expected_blake3 = *blake3::hash(&current).as_bytes();
        let mut manifest = self.open_manifest(&current)?.ok_or_else(|| CoreError::new(ErrorCode::Internal, "vault manifest missing"))?;
        if intents.is_empty() && !manifest.has_pending() { return Ok(None); }
        manifest.settle(|path| self.observe(core, path)).map_err(crypto)?;
        for intent in intents {
            let locator = man::locator(&self.keys, &intent.folder_path, &intent.object).map_err(crypto)?;
            let path = intent.path();
            let applied = match intent.hash { Some(hash) => manifest.begin_change(locator, hash, &path), None => manifest.begin_delete(locator, &path) };
            applied.map_err(|error| match error {
                CryptoError::Busy => CoreError::new(ErrorCode::Integrity, "manifest shows an unexplained change of this entry; check the vault"),
                CryptoError::InvalidInput => CoreError::new(ErrorCode::Integrity, "entry is not recorded in the vault manifest"),
                other => crypto(other),
            })?;
        }
        manifest.sequence = manifest.sequence.checked_add(1).ok_or_else(|| CoreError::new(ErrorCode::LimitExceeded, "manifest sequence exhausted"))?;
        let sealed = man::seal(&self.keys, &manifest, &mut OsRandom).map_err(crypto)?;
        let staged = manifest_spool_path(spool.path)?;
        let _ = spool.storage.delete_file(&staged);
        let prepared = self.spool(Spool { storage: spool.storage, path: &staged }, location.clone(), (String::new(), String::new()),
            |out| out.write_all(&sealed).map_err(|_| CryptoError::Io))?;
        Ok(Some((Operation::ReplaceContentExpected { source: prepared.spool, target: location, bytes: prepared.bytes,
            blake3: prepared.blake3, expected_target: expected, expected_target_blake3: expected_blake3 }, manifest.sequence)))
    }

    fn folder_location(&self, folder: &[u8; 16]) -> Result<Location> {
        child(&self.root, &self.keys.folder_path(folder).map_err(crypto)?)
    }
    fn folder_dirs(&self, folder: &[u8; 16]) -> Result<Vec<Location>> {
        let path = self.keys.folder_path(folder).map_err(crypto)?;
        let parts: Vec<&str> = path.split('/').collect();
        (1..=parts.len()).map(|n| child(&self.root, &parts[..n].join("/"))).collect()
    }
    fn object_location(&self, folder: &[u8; 16], name: &str, kind: EntryKind) -> Result<Location> {
        let object = self.keys.entry_name(folder, name, kind).map_err(crypto)?;
        child(&self.folder_location(folder)?, &object)
    }
    /// Stage ciphertext in the spool (create-new) while hashing it.
    fn spool(&self, spool: Spool<'_>, target: Location, (folder_path, object): (String, String),
        write: impl FnOnce(&mut dyn Write) -> paranoic_crypto::Result<()>) -> Result<PreparedObject> {
        let Spool { storage: spool, path } = spool;
        let mut staged = spool.stage(path, JobId(uuid::Uuid::new_v4()))?;
        let mut hashing = Hashing { inner: &mut *staged, hash: blake3::Hasher::new(), bytes: 0, head: Vec::with_capacity(ENTRY_HEADER_LEN) };
        let written = write(&mut hashing);
        let (bytes, digest) = (hashing.bytes, *hashing.hash.finalize().as_bytes());
        let header = if hashing.head.len() == ENTRY_HEADER_LEN { Some(man::header_hash(&hashing.head).map_err(crypto)?) } else { None };
        match written {
            Ok(()) => staged.publish()?,
            Err(error) => { let _ = staged.abort(); return Err(crypto(error)); }
        }
        Ok(PreparedObject { spool: Location { storage: spool.id(), path: path.clone() }, target, bytes, blake3: digest, folder_path, object, header })
    }
}

/// Hold publication until the caller's source-version checks also succeed.
struct DeferredCommit(std::rc::Rc<std::cell::RefCell<Option<Box<dyn StreamSink>>>>);
impl Write for DeferredCommit {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().as_mut().ok_or_else(|| std::io::Error::other("closed sink"))?.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().as_mut().ok_or_else(|| std::io::Error::other("closed sink"))?.flush()
    }
}
impl StreamSink for DeferredCommit {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> {
        if let Some(sink) = self.0.borrow_mut().take() { sink.abort()?; }
        Ok(())
    }
}

/// Submit a plan step by step, waiting for each job; stops at the first job
/// that does not succeed and returns every record so far.
pub fn run_plan(core: &Core, plan: &VaultPlan, timeout: std::time::Duration) -> Result<Vec<JobRecord>> {
    if plan.needs_manifest() {
        return Err(CoreError::new(ErrorCode::Internal, "vault plan has not passed with_manifest"));
    }
    let mut records = Vec::new();
    for step in &plan.steps {
        let job = core.submit(step.clone())?;
        let until = std::time::Instant::now() + timeout;
        let done = loop {
            let current = core.job(job.id)?;
            if !matches!(current.state, JobState::Queued | JobState::Running) { break current; }
            if std::time::Instant::now() > until { return Err(CoreError::new(ErrorCode::NeedsAttention, "vault plan step still running")); }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let failed = done.state != JobState::Succeeded;
        records.push(done);
        if failed { break; }
    }
    Ok(records)
}

fn expect_success(records: Vec<JobRecord>, steps: usize) -> Result<()> {
    match records.last() {
        Some(last) if records.len() == steps && last.state == JobState::Succeeded => Ok(()),
        Some(last) => Err(last.error.clone().unwrap_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "vault plan step failed"))),
        None => Err(CoreError::new(ErrorCode::Internal, "empty vault plan")),
    }
}

/// Spool writer shared between the re-encrypting sink and its result.
struct Shared { staged: Option<Box<dyn paranoic_core::StagedWrite>>, hash: blake3::Hasher, bytes: u64, failed: bool, head: Vec<u8> }
struct SharedWrite(std::rc::Rc<std::cell::RefCell<Shared>>);
impl Write for SharedWrite {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut shared = self.0.borrow_mut();
        let n = shared.staged.as_mut().ok_or_else(|| std::io::Error::other("spool closed"))?.write(data)?;
        shared.hash.update(&data[..n]); shared.bytes += n as u64;
        let room = ENTRY_HEADER_LEN.saturating_sub(shared.head.len()).min(n);
        shared.head.extend_from_slice(&data[..room]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
/// Receives verified plaintext and re-encrypts it at once; the spool object
/// is published only when the whole source verified (commit).
struct Reencrypt { writer: Option<v1::FileWriter<'static>>, shared: std::rc::Rc<std::cell::RefCell<Shared>> }
impl Write for Reencrypt {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.writer.as_mut().ok_or_else(|| std::io::Error::other("closed"))?.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for Reencrypt {
    fn commit(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        let finished = self.writer.take().ok_or(CryptoError::InvalidInput)?.finish();
        let staged = self.shared.borrow_mut().staged.take().ok_or(CryptoError::InvalidInput)?;
        match finished {
            Ok(()) => staged.publish().map_err(|_| CryptoError::Io),
            Err(error) => { let _ = staged.abort(); self.shared.borrow_mut().failed = true; Err(error) }
        }
    }
    fn abort(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        self.writer.take();
        let mut shared = self.shared.borrow_mut();
        shared.failed = true;
        if let Some(staged) = shared.staged.take() { let _ = staged.abort(); }
        Ok(())
    }
}

struct Hashing<'a> { inner: &'a mut dyn Write, hash: blake3::Hasher, bytes: u64, head: Vec<u8> }
impl Write for Hashing<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(data)?; self.hash.update(&data[..n]); self.bytes += n as u64;
        let room = ENTRY_HEADER_LEN.saturating_sub(self.head.len()).min(n);
        self.head.extend_from_slice(&data[..room]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}

/// Reader that keeps the first `ENTRY_HEADER_LEN` bytes it passes on.
struct HeaderTap<'a, R: Read> { inner: &'a mut R, head: Vec<u8> }
impl<R: Read> Read for HeaderTap<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        let room = ENTRY_HEADER_LEN.saturating_sub(self.head.len()).min(n);
        self.head.extend_from_slice(&buffer[..room]);
        Ok(n)
    }
}
impl PreparedObject {
    fn intent(&self) -> Intent { Intent { folder_path: self.folder_path.clone(), object: self.object.clone(), hash: self.header } }
}

fn child(base: &Location, relative: &str) -> Result<Location> {
    let path = if base.path.is_root() { ResourcePath::parse(relative)? } else { ResourcePath::parse(format!("{}/{relative}", base.path.as_str()))? };
    Ok(Location { storage: base.storage, path })
}
fn read_all(core: &Core, location: &Location, limit: u64) -> Result<Vec<u8>> {
    let mut input = core.open_read(location)?;
    let mut data = Vec::new();
    input.by_ref().take(limit + 1).read_to_end(&mut data).map_err(CoreError::io)?;
    if data.len() as u64 > limit { return Err(CoreError::new(ErrorCode::LimitExceeded, "vault header too large")); }
    Ok(data)
}
fn read_header(core: &Core, location: &Location) -> Result<Vec<u8>> {
    let mut input = core.open_read(location)?;
    let mut header = vec![0u8; ENTRY_HEADER_LEN];
    input.read_exact(&mut header).map_err(|_| CoreError::new(ErrorCode::Integrity, "vault entry truncated"))?;
    Ok(header)
}
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "vault path is not a folder") }
/// Message of the error returned when a password or recovery key does not
/// open the vault header. A wrong key and a damaged header are
/// indistinguishable (AEAD); hosts show it as a wrong password.
pub const WRONG_KEY: &str = "wrong password or recovery key, or damaged vault header";
/// `PermissionDenied` with `WRONG_KEY` for a failed header unlock.
pub fn is_wrong_key(error: &CoreError) -> bool { error.code == ErrorCode::PermissionDenied && error.message == WRONG_KEY }
fn unlock_error(error: CryptoError) -> CoreError {
    match error {
        CryptoError::Authentication => CoreError::new(ErrorCode::PermissionDenied, WRONG_KEY),
        other => crypto(other),
    }
}
fn crypto(error: CryptoError) -> CoreError {
    match error {
        CryptoError::Authentication => CoreError::new(ErrorCode::Integrity, "vault data failed authentication or wrong password"),
        CryptoError::LimitExceeded => CoreError::new(ErrorCode::LimitExceeded, "vault limit exceeded"),
        CryptoError::Cancelled => CoreError::new(ErrorCode::Cancelled, "cancelled"),
        CryptoError::Unsupported => CoreError::new(ErrorCode::Unsupported, "unsupported vault version"),
        CryptoError::InvalidInput => CoreError::new(ErrorCode::InvalidInput, "invalid vault name or data"),
        _ => CoreError::new(ErrorCode::Io, "vault operation failed"),
    }
}

/// Random-access plaintext of one vault file, in memory (`open_reader`).
pub struct VaultFileReader { handle: Box<dyn paranoic_core::ReadHandle>, before: Metadata, cipher: v1::FileCipher }
impl VaultFileReader {
    pub fn size(&self) -> u64 { self.cipher.size() }
    /// Up to `len` bytes from `offset` (fewer at the end). Only the chunks
    /// needed are read and authenticated.
    pub fn read_at(&mut self, offset: u64, len: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let mut out = zeroize::Zeroizing::new(Vec::new());
        if offset >= self.size() || len == 0 { return Ok(out); }
        let end = (offset + len as u64).min(self.size());
        let chunk = v1::FileCipher::CHUNK as u64;
        for index in offset / chunk..=(end - 1) / chunk {
            let (start, sealed_len) = self.cipher.chunk_range(index);
            self.handle.seek_to(start)?;
            let mut sealed = vec![0u8; sealed_len];
            self.handle.read_exact(&mut sealed).map_err(|_| CoreError::new(ErrorCode::Integrity, "vault file truncated"))?;
            let plain = self.cipher.open_chunk(index, &sealed).map_err(crypto)?;
            let from = offset.saturating_sub(index * chunk) as usize;
            let to = ((end - index * chunk) as usize).min(plain.len());
            out.extend_from_slice(&plain[from.min(to)..to]);
        }
        if self.handle.metadata()? != self.before {
            return Err(CoreError::new(ErrorCode::SourceChanged, "vault file changed while reading"));
        }
        Ok(out)
    }
}
