//! `CloudStorage`: a cloud folder as a Core `Storage` grant (roadmap 3.7).
//! Every Core operation (copy/move/delete trees, verified transfers, journal,
//! cancellation, recovery, Cryptomator vault jobs) then works between local
//! storage and the cloud, or between clouds, through the durable Core queue.
//!
//! Mapping and guarantees:
//! * paths are resolved component by component with exact-name lookups;
//!   duplicate names (Drive) make the path ambiguous → Unsupported, never a guess;
//! * listings are complete or fail: a non-portable or duplicate name fails the
//!   listing (Core copies/deletes trees and must not silently skip entries);
//!   adapter staging objects (".paranoic-*") are hidden like on local storage;
//! * `stage` spools to a private local file, then uploads create-new at
//!   publish (the final name appears only when the upload completed);
//! * `publish_tree` / `stage_bundle` build a hidden ".paranoic-<job>" folder and
//!   expose it with one relocate (atomic on Drive/OneDrive);
//! * deletion moves to the provider trash (recoverable); folders must be empty;
//! * `move_no_replace` is offered only where the provider relocates atomically
//!   without replacement (Drive, OneDrive); pCloud reports Unsupported.
use std::{fs::File, io::{Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicU64, Ordering}}};
use paranoic_core::{Capabilities, DirectoryEntry, EntryKind, JobId, Location, Metadata, MoveSource, PinnedDestination,
    PreparedTreeFile, ReadHandle, ResourcePath, StagedWrite, Storage, StorageDomain, StorageId};
use crate::{CloudEntry, CloudId, CloudKind, CloudProvider, CoreError, ErrorCode, Result};

pub struct CloudStorage { id: StorageId, provider: Arc<dyn CloudProvider>, root: CloudId, account: String, spool: PathBuf }

fn not_found() -> CoreError { CoreError::new(ErrorCode::NotFound, "cloud path not found") }
fn to_meta(entry: &CloudEntry) -> Metadata {
    Metadata { kind: match entry.kind { CloudKind::File => EntryKind::File, CloudKind::Folder => EntryKind::Directory, CloudKind::Native => EntryKind::Other },
        bytes: entry.bytes.unwrap_or(0), modified_unix_ms: entry.modified_unix_ms }
}
fn resolve(provider: &dyn CloudProvider, root: &CloudId, path: &ResourcePath) -> Result<CloudEntry> {
    let mut current = provider.metadata(root)?;
    if path.is_root() { return if current.kind == CloudKind::Folder { Ok(current) } else { Err(not_found()) }; }
    for part in path.as_str().split('/') {
        if current.kind != CloudKind::Folder { return Err(not_found()); }
        current = provider.lookup(&current.id, part)?.ok_or_else(not_found)?;
    }
    Ok(current)
}

impl CloudStorage {
    /// Provider revision (ETag / version id) of every file below `path`,
    /// keyed by the path relative to `path`. One walk; folders are bounded.
    /// Used by sync to see content changes that keep size and mtime.
    pub fn revisions(&self, path: &ResourcePath, max_entries: usize) -> Result<std::collections::BTreeMap<String, String>> {
        let mut out = std::collections::BTreeMap::new();
        let start = resolve(self.provider.as_ref(), &self.root, path)?;
        let mut stack = vec![(start, String::new(), 0usize)];
        let mut seen = 0usize;
        while let Some((folder, prefix, depth)) = stack.pop() {
            if depth > 64 { return Err(CoreError::new(ErrorCode::LimitExceeded, "cloud tree too deep")); }
            for entry in self.provider.list_all(&folder.id, max_entries)? {
                seen += 1;
                if seen > max_entries { return Err(CoreError::new(ErrorCode::LimitExceeded, "cloud tree exceeds listing limit")); }
                if entry.name.starts_with(".paranoic-") { continue; }
                let rel = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
                match entry.kind {
                    CloudKind::Folder => stack.push((entry, rel, depth + 1)),
                    CloudKind::File => { if let Some(revision) = entry.revision { out.insert(rel, revision); } }
                    CloudKind::Native => {}
                }
            }
        }
        Ok(out)
    }

    /// Current provider revision of one file (None if the provider has none).
    pub fn revision(&self, path: &ResourcePath) -> Result<Option<String>> {
        Ok(resolve(self.provider.as_ref(), &self.root, path)?.revision)
    }

    /// `root` is the granted cloud folder. `spool_dir` is an existing
    /// app-private directory (0700 on Unix), excluded from backup, used for
    /// pending uploads; it is never a Files grant and never Private Vault.
    pub fn open(id: StorageId, provider: Arc<dyn CloudProvider>, root: CloudId, spool_dir: impl AsRef<Path>) -> Result<Self> {
        let spool = std::fs::canonicalize(spool_dir.as_ref()).map_err(CoreError::io)?;
        let info = std::fs::metadata(&spool).map_err(CoreError::io)?;
        if !info.is_dir() { return Err(CoreError::new(ErrorCode::InvalidInput, "cloud spool must be a directory")); }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if info.permissions().mode() & 0o077 != 0 { return Err(CoreError::new(ErrorCode::PermissionDenied, "cloud spool must be private (0700)")); }
        }
        let account = provider.account_id()?;
        if provider.metadata(&root)?.kind != CloudKind::Folder { return Err(CoreError::new(ErrorCode::InvalidInput, "cloud grant root must be a folder")); }
        Ok(Self { id, provider, root, account, spool })
    }
    pub fn provider(&self) -> &Arc<dyn CloudProvider> { &self.provider }
    /// Current provider entry for a path (hashes, revision) — used by sync.
    pub fn remote_entry(&self, path: &ResourcePath) -> Result<CloudEntry> { resolve(self.provider.as_ref(), &self.root, path) }
    /// Remove spool files left by a crash. Call only while no stage of this
    /// storage can be active: before it is registered with Core. See
    /// [`purge_spool_dir`].
    pub fn purge_spool(&self) -> Result<u64> { purge_spool_dir(&self.spool) }
    fn parent(&self, path: &ResourcePath) -> Result<(CloudEntry, String)> {
        let (parent, name) = path.parent_and_name()?;
        let folder = resolve(self.provider.as_ref(), &self.root, &parent)?;
        if folder.kind != CloudKind::Folder { return Err(not_found()); }
        Ok((folder, name.to_owned()))
    }
    /// Strict (Core operations) or lenient (display: omit and count) listing.
    fn entries(&self, path: &ResourcePath, limit: usize, lenient: bool) -> Result<paranoic_core::PartialListing> {
        if !(1..=100_000).contains(&limit) { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid directory listing limit")); }
        let folder = resolve(self.provider.as_ref(), &self.root, path)?;
        if folder.kind != CloudKind::Folder { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud folder")); }
        // One snapshot per traversal; internal staging objects are hidden.
        let all: Vec<CloudEntry> = self.provider.list_all(&folder.id, limit.saturating_add(1024))?
            .into_iter().filter(|e| !crate::is_internal(&e.name)).collect();
        if all.len() > limit { return Err(CoreError::new(ErrorCode::LimitExceeded, "directory exceeds listing limit")); }
        let mut counts = std::collections::HashMap::new();
        for e in &all { *counts.entry(e.name.clone()).or_insert(0usize) += 1; }
        let (mut entries, mut skipped) = (Vec::new(), 0u64);
        for entry in all {
            let joined = path.join(&entry.name);
            if counts.get(&entry.name) != Some(&1) || joined.is_err() {
                if lenient { skipped += 1; continue; }
                return Err(CoreError::new(ErrorCode::Unsupported, "cloud folder contains duplicate or non-portable names"));
            }
            entries.push(DirectoryEntry { location: Location { storage: self.id, path: joined? }, metadata: to_meta(&entry) });
        }
        entries.sort_by(|a, b| a.location.path.as_str().cmp(b.location.path.as_str()));
        Ok(paranoic_core::PartialListing { entries, skipped, unknown_size: Vec::new() })
    }
    fn atomic(&self) -> Result<()> {
        if self.provider.capabilities().atomic_relocate { Ok(()) }
        else { Err(CoreError::new(ErrorCode::Unsupported, "provider cannot move atomically without replacement")) }
    }
    fn reader(&self, entry: CloudEntry) -> CloudReader { CloudReader { provider: Arc::clone(&self.provider), entry, stream: None, offset: 0 } }
}

impl Storage for CloudStorage {
    fn id(&self) -> StorageId { self.id }
    fn binding_key(&self) -> Result<String> { Ok(format!("cloud:{:?}:{}:{}", self.provider.kind(), self.account, self.root.as_str())) }
    fn provider_revision(&self, path: &ResourcePath) -> Result<Option<String>> { CloudStorage::revision(self, path) }
    /// One conditional request with the PLANNED revision (WebDAV: `If-Match`),
    /// not the one found now: a version written meanwhile is not deleted.
    fn delete_file_at_revision(&self, path: &ResourcePath, revision: &str) -> Result<()> {
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud file")); }
        if entry.revision.as_deref() != Some(revision) { return Err(crate::stale()); }
        self.provider.trash(&entry)
    }
    fn legacy_binding_keys(&self) -> Result<Vec<String>> {
        Ok(self.provider.legacy_account_ids().into_iter()
            .map(|account| format!("cloud:{:?}:{}:{}", self.provider.kind(), account, self.root.as_str())).collect())
    }
    fn domain(&self) -> StorageDomain { StorageDomain::Files }
    fn capabilities(&self) -> Capabilities {
        // Only what the provider actually guarantees: deletion only where it is
        // recoverable (trash); source/destination pinning only where mutations
        // are conditional on the exact version on the server (OneDrive, WebDAV
        // with ETags). Drive/pCloud therefore copy but never move-with-delete.
        let p = self.provider.capabilities();
        Capabilities { read: true, write: true, delete: p.trash, atomic_move_no_replace: p.atomic_relocate,
            durable_directory_sync: true, conditional_move_source: p.conditional_mutation && p.trash,
            pinned_destination: p.conditional_mutation, directory_sync_waived: false }
    }
    fn metadata(&self, path: &ResourcePath) -> Result<Metadata> { Ok(to_meta(&resolve(self.provider.as_ref(), &self.root, path)?)) }
    fn list(&self, path: &ResourcePath, limit: usize) -> Result<Vec<DirectoryEntry>> { Ok(self.entries(path, limit, false)?.entries) }
    fn list_for_display(&self, path: &ResourcePath, limit: usize) -> Result<paranoic_core::PartialListing> { self.entries(path, limit, true) }
    fn remote_publication(&self) -> bool { true }
    fn open_read(&self, path: &ResourcePath) -> Result<Box<dyn ReadHandle>> {
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a downloadable cloud file")); }
        Ok(Box::new(self.reader(entry)))
    }
    fn stage(&self, target: &ResourcePath, _job: JobId) -> Result<Box<dyn StagedWrite>> {
        // No existence check here: Core's Replace stages first and moves the old
        // file aside only before publish. Publication itself is create-new.
        let (parent, name) = self.parent(target)?;
        Ok(Box::new(CloudStage { provider: Arc::clone(&self.provider), parent: parent.id, name, spool: Spool::create(&self.spool)?, bundle: None }))
    }
    fn stage_bundle(&self, target: &ResourcePath, job: JobId, content_name: &str, sidecar_name: &str, sidecar: &[u8]) -> Result<Box<dyn StagedWrite>> {
        crate::validate_name(content_name)?; crate::validate_name(sidecar_name)?;
        if content_name == sidecar_name || sidecar.len() > 65_536 { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid bundle layout")); }
        let (parent, name) = self.parent(target)?;
        if self.provider.lookup(&parent.id, &name)?.is_some() { return Err(crate::exists()); }
        Ok(Box::new(CloudStage { provider: Arc::clone(&self.provider), parent: parent.id, name, spool: Spool::create(&self.spool)?,
            bundle: Some(Bundle { job, content: content_name.to_owned(), sidecar_name: sidecar_name.to_owned(), sidecar: sidecar.to_vec() }) }))
    }
    fn publish_tree(&self, target: &ResourcePath, job: JobId, files: &[PreparedTreeFile], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        paranoic_core::validate_prepared_tree(files)?;
        checkpoint()?;
        let (parent, name) = self.parent(target)?;
        if self.provider.lookup(&parent.id, &name)?.is_some() { return Err(crate::exists()); }
        let provider = self.provider.as_ref();
        let staging = provider.create_folder(&parent.id, &staging_name(job))?;
        let built = (|| {
            let mut folders: std::collections::BTreeMap<String, CloudId> = std::collections::BTreeMap::new();
            for file in files {
                checkpoint()?;
                let (dir, leaf) = file.path.parent_and_name()?;
                let mut current = staging.id.clone(); let mut prefix = String::new();
                for part in dir.as_str().split('/').filter(|p| !p.is_empty()) {
                    prefix = if prefix.is_empty() { part.to_owned() } else { format!("{prefix}/{part}") };
                    current = match folders.get(&prefix) { Some(id) => id.clone(), None => {
                        let id = provider.create_folder(&current, part)?.id; folders.insert(prefix.clone(), id.clone()); id } };
                }
                let mut input = std::io::Cursor::new(&file.bytes[..]);
                provider.upload_new(&current, leaf, file.bytes.len() as u64, &mut input, &mut |_| Ok(()))?;
            }
            checkpoint()?;
            provider.metadata(&staging.id)
        })();
        let fresh = match built { Ok(entry) => entry, Err(error) => { discard(provider, &staging.id)?; return Err(error); } };
        // Publication point: after a successful relocate never report Cancelled.
        match provider.relocate(&fresh, None, Some(&name)) {
            Ok(_) => Ok(()),
            Err(error) if error.code == ErrorCode::AlreadyExists || error.code == ErrorCode::SourceChanged => { discard(provider, &staging.id)?; Err(error) }
            Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "cloud tree publication outcome unknown")),
        }
    }
    fn rename_mapped(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8]) -> Result<()> {
        self.atomic()?;
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::rename(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, job, old, new)
    }
    fn inspect_mapped_rename(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8],
        checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::inspect(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, job, old, new, checkpoint)
    }
    fn recover_mapped(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId, old: &[u8], new: &[u8],
        action: paranoic_core::MappedRecoveryAction) -> Result<()> {
        self.atomic()?;
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::recover(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, original, recovery, old, new, action)
    }
    fn reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &paranoic_core::DirectoryReshape) -> Result<()> {
        self.atomic()?;
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::reshape(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, job, layout)
    }
    fn inspect_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &paranoic_core::DirectoryReshape,
        checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::inspect_reshape(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, job, layout, checkpoint)
    }
    fn recover_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId,
        layout: &paranoic_core::DirectoryReshape, action: paranoic_core::MappedRecoveryAction) -> Result<()> {
        self.atomic()?;
        let ((parent, name), (target_parent, target_name)) = (self.parent(source)?, self.parent(target)?);
        crate::mapped::recover_reshape(self.provider.as_ref(), &parent, &name, &target_parent, &target_name, original, recovery, layout, action)
    }
    fn create_directory(&self, path: &ResourcePath) -> Result<()> {
        let (parent, name) = self.parent(path)?;
        self.provider.create_folder(&parent.id, &name).map(|_| ())
    }
    fn move_no_replace(&self, source: &ResourcePath, target: &ResourcePath) -> Result<()> {
        if !self.provider.capabilities().atomic_relocate {
            return Err(CoreError::new(ErrorCode::Unsupported, "provider cannot move atomically without replacement"));
        }
        let entry = resolve(self.provider.as_ref(), &self.root, source)?;
        let (parent, name) = self.parent(target)?;
        let same_parent = entry.parents.contains(&parent.id);
        let rename = (entry.name != name).then_some(name.as_str());
        self.provider.relocate(&entry, (!same_parent).then_some(&parent.id), rename).map(|_| ())
    }
    fn delete_file(&self, path: &ResourcePath) -> Result<()> {
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud file")); }
        self.provider.trash(&entry)
    }
    fn delete_empty_directory(&self, path: &ResourcePath) -> Result<()> {
        if path.is_root() { return Err(CoreError::new(ErrorCode::InvalidPath, "cannot delete the cloud grant root")); }
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::Folder { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud folder")); }
        self.provider.trash(&entry)
    }
    fn prepare_move_source(&self, path: &ResourcePath) -> Result<Box<dyn MoveSource>> {
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud file")); }
        Ok(Box::new(CloudMoveSource { reader: self.reader(entry) }))
    }
    fn pin_destination(&self, path: &ResourcePath) -> Result<Box<dyn PinnedDestination>> {
        let entry = resolve(self.provider.as_ref(), &self.root, path)?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a cloud file")); }
        Ok(Box::new(CloudPinned { reader: self.reader(entry), root: self.root.clone(), path: path.clone() }))
    }
}

fn staging_name(job: JobId) -> String { format!(".paranoic-{}", job.0.simple()) }

/// Trash an adapter-owned staging folder bottom-up (bounded depth). Failure is
/// NeedsAttention: the hidden folder stays for inspection, never re-used.
fn discard(provider: &dyn CloudProvider, folder: &CloudId) -> Result<()> {
    fn walk(provider: &dyn CloudProvider, folder: &CloudId, depth: usize) -> Result<()> {
        if depth > 16 { return Err(CoreError::new(ErrorCode::LimitExceeded, "staging too deep")); }
        // Audit A14: entries are removed as we go, so an offset token would skip the next page.
        // Always ask for the first page again, until it is empty (bounded number of rounds).
        for _ in 0..10_000 {
            let chunk = provider.list(folder, None, 1000)?;
            if chunk.entries.is_empty() { return provider.trash(&provider.metadata(folder)?); }
            for entry in chunk.entries {
                match entry.kind {
                    CloudKind::Folder => walk(provider, &entry.id, depth + 1)?,
                    _ => provider.trash(&entry)?,
                }
            }
        }
        Err(CoreError::new(ErrorCode::LimitExceeded, "staging cleanup does not converge"))
    }
    walk(provider, folder, 0).map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "cloud staging cleanup incomplete"))
}

/// Upload as `.paranoic-up-<id>` in `parent`, then rename to `name` (no
/// replace). On failure or cancellation the temporary object is removed; if
/// that cannot be confirmed the result is NeedsAttention (hidden leftover).
fn upload_into_place(provider: &dyn CloudProvider, parent: &CloudId, name: &str, bytes: u64, input: &mut dyn Read,
    checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<()> {
    // Ordinary conflicts are reported before any byte is sent (the rename
    // below stays no-replace for the remaining race).
    if provider.lookup(parent, name)?.is_some() { return Err(crate::exists()); }
    let temporary = format!(".paranoic-up-{}", uuid::Uuid::new_v4().simple());
    let leftover = || CoreError::new(ErrorCode::NeedsAttention, "interrupted cloud upload left a hidden temporary file");
    let remove = |entry: &CloudEntry| provider.trash(entry).map_err(|_| leftover());
    let uploaded = provider.upload_new(parent, &temporary, bytes, input, checkpoint)
        .and_then(|entry| { checkpoint(bytes)?; Ok(entry) });
    let entry = match uploaded {
        Ok(entry) => entry,
        Err(error) => {
            // The server may have kept a partial body: find and remove it.
            return match provider.lookup(parent, &temporary) {
                Ok(None) => Err(error),
                Ok(Some(partial)) => { remove(&partial)?; Err(error) }
                Err(_) => Err(leftover()),
            };
        }
    };
    // Publication point: after a successful rename never report Cancelled.
    match provider.relocate(&entry, None, Some(name)) {
        Ok(_) => Ok(()),
        Err(error) if error.code == ErrorCode::AlreadyExists || error.code == ErrorCode::SourceChanged => { remove(&entry)?; Err(error) }
        Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "cloud upload publication outcome unknown")),
    }
}

/// Process-wide bound on local spool bytes of all pending cloud uploads
/// (default 4 GiB). A write beyond it fails the job instead of filling the disk.
static SPOOL_USED: AtomicU64 = AtomicU64::new(0);
static SPOOL_LIMIT: AtomicU64 = AtomicU64::new(4 << 30);
/// Host setting for the spool bound (e.g. from free space); applies to new writes.
pub fn set_spool_limit(bytes: u64) { SPOOL_LIMIT.store(bytes, Ordering::Relaxed); }
fn reserve_spool(bytes: u64) -> bool {
    let limit = SPOOL_LIMIT.load(Ordering::Relaxed);
    SPOOL_USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| used.checked_add(bytes).filter(|total| *total <= limit)).is_ok()
}

/// Remove upload spool files (`.paranoic-spool-*`) left in `directory` by a
/// crash (У-39). A spool is a private copy of data whose source still exists,
/// and its random name is never journaled, so after a restart nothing can
/// resume from it. It may hold unencrypted user data: a file that cannot be
/// removed is emptied; if even that fails its size stays counted against the
/// spool limit so repeated crashes cannot exceed it. Returns the bytes left.
/// Only files directly in `directory` are touched; links are not followed.
pub fn purge_spool_dir(directory: &Path) -> Result<u64> {
    let mut left = 0u64;
    for item in std::fs::read_dir(directory).map_err(CoreError::io)? {
        let Ok(item) = item else { continue };
        if !item.file_name().to_string_lossy().starts_with(".paranoic-spool-") { continue; }
        if !item.file_type().is_ok_and(|kind| kind.is_file()) { continue; }
        let path = item.path();
        if std::fs::remove_file(&path).is_ok() { continue; }
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) { let _ = file.set_len(0); }
        let bytes = std::fs::symlink_metadata(&path).map(|m| m.len()).unwrap_or(0);
        if bytes > 0 {
            SPOOL_USED.fetch_add(bytes, Ordering::AcqRel);
            left = left.saturating_add(bytes);
        }
    }
    Ok(left)
}

struct Spool { path: PathBuf, file: Option<File>, written: u64 }
impl Spool {
    fn create(directory: &Path) -> Result<Self> {
        let path = directory.join(format!(".paranoic-spool-{}", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let file = options.open(&path).map_err(CoreError::io)?;
        Ok(Self { path, file: Some(file), written: 0 })
    }
    fn rewind(&mut self) -> Result<&mut File> {
        let file = self.file.as_mut().ok_or_else(|| CoreError::new(ErrorCode::Internal, "closed cloud spool"))?;
        file.flush().map_err(CoreError::io)?;
        file.seek(SeekFrom::Start(0)).map_err(CoreError::io)?;
        Ok(file)
    }
}
impl Spool {
    /// Remove the spool file. The space is released from the process limit only once the
    /// file is confirmed gone (audit A15); a failure stays visible to the caller.
    fn remove(&mut self) -> std::io::Result<()> {
        self.file.take();
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let written = std::mem::take(&mut self.written);
        let _ = SPOOL_USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| Some(used.saturating_sub(written)));
        Ok(())
    }
}
impl Drop for Spool {
    fn drop(&mut self) {
        // Fallback only; a leftover keeps counting against the limit until the start-up sweep.
        let _ = self.remove();
    }
}

struct Bundle { job: JobId, content: String, sidecar_name: String, sidecar: Vec<u8> }
struct CloudStage { provider: Arc<dyn CloudProvider>, parent: CloudId, name: String, spool: Spool, bundle: Option<Bundle> }
impl Write for CloudStage {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let file = self.spool.file.as_mut().ok_or_else(|| std::io::Error::other("closed cloud spool"))?;
        if !reserve_spool(data.len() as u64) { return Err(std::io::Error::new(std::io::ErrorKind::StorageFull, "cloud upload spool limit")); }
        match file.write(data) {
            Ok(n) => {
                // Return the unused part of the reservation.
                let unused = (data.len() - n) as u64;
                let _ = SPOOL_USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| Some(used.saturating_sub(unused)));
                self.spool.written += n as u64; Ok(n)
            }
            Err(error) => {
                let _ = SPOOL_USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| Some(used.saturating_sub(data.len() as u64)));
                Err(error)
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> { self.spool.file.as_mut().ok_or_else(|| std::io::Error::other("closed cloud spool"))?.flush() }
}
impl StagedWrite for CloudStage {
    fn publish(self: Box<Self>) -> Result<()> { self.publish_checked(&mut |_| Ok(())) }
    /// Network upload under Core's checkpoint (progress + cancellation, У-22).
    /// The file is uploaded under a hidden temporary name and renamed into
    /// place (no replace) only when complete, so an interrupted or cancelled
    /// upload never leaves a truncated file under the target name.
    fn publish_checked(mut self: Box<Self>, checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<()> {
        let bytes = self.spool.written;
        let provider = Arc::clone(&self.provider);
        let parent = self.parent.clone(); let name = self.name.clone();
        let bundle = self.bundle.take();
        let file = self.spool.rewind()?;
        let mut input = file.take(bytes);
        let Some(bundle) = bundle else {
            return upload_into_place(provider.as_ref(), &parent, &name, bytes, &mut input, checkpoint);
        };
        let staging = provider.create_folder(&parent, &staging_name(bundle.job))?;
        let built = (|| {
            provider.upload_new(&staging.id, &bundle.content, bytes, &mut input, checkpoint)?;
            let mut sidecar = std::io::Cursor::new(&bundle.sidecar[..]);
            provider.upload_new(&staging.id, &bundle.sidecar_name, bundle.sidecar.len() as u64, &mut sidecar, &mut |_| Ok(()))?;
            checkpoint(bytes)?;
            provider.metadata(&staging.id)
        })();
        let fresh = match built { Ok(entry) => entry, Err(error) => { discard(provider.as_ref(), &staging.id)?; return Err(error); } };
        match provider.relocate(&fresh, None, Some(&name)) {
            Ok(_) => Ok(()),
            Err(error) if error.code == ErrorCode::AlreadyExists || error.code == ErrorCode::SourceChanged => {
                discard(provider.as_ref(), &staging.id)?; Err(error) }
            Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "cloud bundle publication outcome unknown")),
        }
    }
    fn abort(mut self: Box<Self>) -> Result<()> {
        // Nothing reached the cloud; the plaintext spool must be confirmed gone (audit A15).
        self.spool.remove().map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "cloud upload spool could not be removed"))
    }
}

struct CloudReader { provider: Arc<dyn CloudProvider>, entry: CloudEntry, stream: Option<Box<dyn Read + Send>>, offset: u64 }
impl Read for CloudReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.stream.is_none() {
            if self.entry.bytes.is_some_and(|b| self.offset >= b) { return Ok(0); }
            self.stream = Some(self.provider.open_download(&self.entry.id, self.offset).map_err(|_| std::io::Error::other("cloud download failed"))?);
        }
        let n = self.stream.as_mut().ok_or_else(|| std::io::Error::other("closed cloud stream"))?.read(buffer)?;
        self.offset += n as u64; Ok(n)
    }
}
impl ReadHandle for CloudReader {
    /// Metadata of the opened object by id (not by re-resolving the path).
    fn metadata(&self) -> Result<Metadata> { Ok(to_meta(&self.provider.metadata(&self.entry.id)?)) }
    fn seek_to(&mut self, offset: u64) -> Result<()> { self.stream = None; self.offset = offset; Ok(()) }
}

/// Removal = trash of exactly the opened object, re-checked against its
/// revision (OneDrive: If-Match; others: check-then-trash, residual race).
struct CloudMoveSource { reader: CloudReader }
impl MoveSource for CloudMoveSource {
    fn reader(&mut self) -> &mut dyn ReadHandle { &mut self.reader }
    fn remove(self: Box<Self>) -> Result<()> { self.reader.provider.trash(&self.reader.entry) }
}
struct CloudPinned { reader: CloudReader, root: CloudId, path: ResourcePath }
impl PinnedDestination for CloudPinned {
    fn reader(&mut self) -> &mut dyn ReadHandle { &mut self.reader }
    fn still_published(&mut self) -> Result<()> {
        let current = resolve(self.reader.provider.as_ref(), &self.root, &self.path)?;
        if current.id == self.reader.entry.id && current.revision == self.reader.entry.revision { Ok(()) }
        else { Err(CoreError::new(ErrorCode::SourceChanged, "cloud destination changed before source removal")) }
    }
}
