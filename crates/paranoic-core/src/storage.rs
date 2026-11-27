use std::{collections::HashMap, io::{Read, Write}, sync::{Arc, RwLock}};

use serde::{Deserialize, Serialize};

use crate::{CoreError, DirectoryEntry, ErrorCode, JobId, Metadata, ResourcePath, Result, StorageId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageDomain { Files, PrivateVault }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
    pub atomic_move_no_replace: bool,
    /// Whether the adapter can flush directory namespace changes to storage.
    pub durable_directory_sync: bool,
    #[serde(default)]
    pub conditional_move_source: bool,
    #[serde(default)]
    pub pinned_destination: bool,
    /// Windows folders can publish vault changes without a directory sync.
    /// Core waives `durable_directory_sync` for publishing NEW trees
    /// (`PublishTree`, `PublishLinkedTrees`, `CopyExpectedBundle`) and for
    /// mapped renames / directory reshapes, whose staging keeps the former
    /// mapping until success so inspection and recovery still work; vault
    /// formats may accept it for their in-vault writes. Cross-storage moves
    /// keep the strict requirement.
    #[serde(default)]
    pub directory_sync_waived: bool,
}

/// Source lease for destructive cross-storage transfers. The adapter pins the
/// source object it opened and remove() must delete only that object: it
/// fails (SourceChanged) if the name now refers to another object or the
/// object was modified since the lease started. Adapters that can only check
/// (not lock) state this residual race in their documentation; writers that
/// already hold an open handle can still modify an unlinked object, as with
/// any file manager move. Drop releases the lease without removing the source.
pub trait MoveSource: Send {
    fn reader(&mut self) -> &mut dyn ReadHandle;
    fn remove(self: Box<Self>) -> Result<()>;
}

/// Read handle on the published destination. still_published() must confirm,
/// immediately before the source is removed, that the destination name still
/// refers to the verified, unmodified object; Core calls it right before
/// MoveSource::remove. Adapters able to prevent unlinking may return Ok always.
pub trait PinnedDestination: Send {
    fn reader(&mut self) -> &mut dyn ReadHandle;
    fn still_published(&mut self) -> Result<()> { Ok(()) }
}

pub trait ReadHandle: Read + Send {
    /// Read metadata from the opened object, not by resolving the path again.
    fn metadata(&self) -> Result<Metadata>;
    /// Optional random access on the SAME opened object (media streaming).
    /// Adapters without it return Unsupported; callers reopen and skip.
    fn seek_to(&mut self, _offset: u64) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "random access unavailable"))
    }
}

/// A provider must reserve a new temporary object exclusively. Publication MUST
/// be atomic and must never replace an existing entry, including a symlink.
/// Dropping an unpublished writer should clean up its own temporary object.
/// After a crash, artifacts are retained for explicit recovery, not auto-retried.
pub trait StagedWrite: Write + Send {
    /// AlreadyExists may only be returned after proving no publication occurred
    /// and cleaning up this writer's temporary objects.
    fn publish(self: Box<Self>) -> Result<()>;
    /// Publication that may take long (network upload): `checkpoint(sent)` is
    /// called as it progresses; an error from it stops publication. Contract:
    /// on any error nothing is published under the target name (or the error
    /// is NeedsAttention). Default: plain `publish` (local adapters).
    fn publish_checked(self: Box<Self>, _checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<()> { self.publish() }
    /// Explicitly discard an unpublished writer. An error means cleanup could
    /// not be confirmed and must remain visible as NeedsAttention.
    fn abort(self: Box<Self>) -> Result<()>;
    /// Best effort: set the modification time of the unpublished object.
    fn set_modified(&mut self, _unix_ms: u64) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "timestamps unavailable"))
    }
}

/// Storage adapters own permission checks and confinement. All paths must stay
/// inside the granted root even when another process changes symlinks. A backend
/// that cannot provide a guarantee returns Unsupported instead of weakening it.
pub trait Storage: Send + Sync {
    fn id(&self) -> StorageId;
    /// Stable identity of the actual granted root, not a display label. The
    /// journal stores its digest and refuses to rebind an existing StorageId.
    fn binding_key(&self) -> Result<String>;
    /// Earlier formats of `binding_key` for the SAME root, computed from the
    /// same grant parameters (identity format upgrades, e.g. WebDAV v1 → v2).
    /// A journal binding equal to one of them is upgraded once to the current
    /// key at registration; any other mismatch is still refused.
    fn legacy_binding_keys(&self) -> Result<Vec<String>> { Ok(Vec::new()) }
    /// Provider revision of a file (cloud ETag), if the storage has one.
    /// Jobs that carry an expected revision compare it with this (У-24).
    fn provider_revision(&self, _path: &ResourcePath) -> Result<Option<String>> { Ok(None) }
    /// Delete the file only while it is still at provider `revision`. Cloud
    /// storages make this one conditional request; the default checks first
    /// and then deletes (storages without revisions never get one).
    fn delete_file_at_revision(&self, path: &ResourcePath, revision: &str) -> Result<()> {
        if self.provider_revision(path)?.as_deref() != Some(revision) {
            return Err(crate::CoreError::new(crate::ErrorCode::SourceChanged, "file changed since it was planned; not deleted"));
        }
        self.delete_file(path)
    }
    fn domain(&self) -> StorageDomain;
    fn capabilities(&self) -> Capabilities;
    /// True when `StagedWrite::publish` transfers the content again (network
    /// upload). Core then counts publication as a second half of the job's
    /// progress, so `bytes_done` keeps growing during the upload.
    fn remote_publication(&self) -> bool { false }
    fn metadata(&self, path: &ResourcePath) -> Result<Metadata>;
    /// Return the complete observed directory, or LimitExceeded if it cannot
    /// fit. Never return a silently truncated prefix as a successful listing.
    /// Each entry must be a direct child of path on this storage ID.
    fn list(&self, path: &ResourcePath, limit: usize) -> Result<Vec<DirectoryEntry>>;
    /// Display-only listing that omits (and counts) entries a strict `list`
    /// would reject: non-portable or non-UTF-8 names, ambiguous duplicates.
    fn list_for_display(&self, path: &ResourcePath, limit: usize) -> Result<PartialListing> {
        Ok(PartialListing { entries: self.list(path, limit)?, skipped: 0 })
    }
    fn open_read(&self, path: &ResourcePath) -> Result<Box<dyn ReadHandle>>;
    fn stage(&self, target: &ResourcePath, job: JobId) -> Result<Box<dyn StagedWrite>>;
    /// Stage a directory containing one streamed file and one bounded sidecar.
    /// publish must expose the complete directory atomically without replacement.
    fn stage_bundle(&self, _target: &ResourcePath, _job: JobId, _content_name: &str,
        _sidecar_name: &str, _sidecar: &[u8]) -> Result<Box<dyn StagedWrite>> {
        Err(CoreError::new(ErrorCode::Unsupported, "atomic bundle publication unavailable"))
    }
    /// Publish a bounded, complete tree without replacement. Check cancellation
    /// during preparation and immediately before publication. After publication,
    /// never return Cancelled or clean through handles to the public tree.
    /// On preparation failure, explicitly clean owned artifacts or report failure.
    fn publish_tree(&self, _target: &ResourcePath, _job: JobId,
        _files: &[crate::PreparedTreeFile], _checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "atomic bootstrap publication unavailable"))
    }
    fn create_directory(&self, path: &ResourcePath) -> Result<()>;
    fn move_no_replace(&self, source: &ResourcePath, target: &ResourcePath) -> Result<()>;
    /// Move a wrapper through private recovery staging and replace name.c9s.
    /// After detaching source, failures must retain payload and never be Cancelled.
    fn rename_mapped(&self, _source: &ResourcePath, _target: &ResourcePath, _job: JobId,
        _old: &[u8], _new: &[u8]) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "mapped rename unavailable"))
    }
    /// Read-only access to this adapter's reserved rename recovery objects.
    fn inspect_mapped_rename(&self, _source: &ResourcePath, _target: &ResourcePath, _job: JobId,
        _old: &[u8], _new: &[u8], _checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<crate::MappedRenameObservation> {
        Err(CoreError::new(ErrorCode::Unsupported, "mapped rename inspection unavailable"))
    }
    /// Recover only the original job's retained payload. Both public names
    /// must be absent. Retain all artifacts on failure; never overwrite.
    #[allow(clippy::too_many_arguments)] // flat, journal-mirrored signature
    fn recover_mapped(&self, _source: &ResourcePath, _target: &ResourcePath,
        _original: JobId, _recovery: JobId, _old: &[u8], _new: &[u8],
        _action: crate::MappedRecoveryAction) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "mapped recovery unavailable"))
    }
    fn reshape_directory(&self, _source: &ResourcePath, _target: &ResourcePath, _job: JobId,
        _layout: &crate::DirectoryReshape) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "directory reshape unavailable"))
    }
    /// Read-only observation of a ReshapeDirectory job's source, target and
    /// retained staging; OldMapping/NewMapping mean old/new wrapper layout.
    fn inspect_reshape_directory(&self, _source: &ResourcePath, _target: &ResourcePath, _job: JobId,
        _layout: &crate::DirectoryReshape, _checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<crate::MappedRenameObservation> {
        Err(CoreError::new(ErrorCode::Unsupported, "directory reshape inspection unavailable"))
    }
    /// Publish the original job's retained wrapper with the requested layout.
    /// Both public names must be absent. Retain all artifacts on failure.
    #[allow(clippy::too_many_arguments)] // flat, journal-mirrored signature
    fn recover_reshape_directory(&self, _source: &ResourcePath, _target: &ResourcePath,
        _original: JobId, _recovery: JobId, _layout: &crate::DirectoryReshape,
        _action: crate::MappedRecoveryAction) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "directory reshape recovery unavailable"))
    }
    /// Best effort: set a directory's modification time (copy preservation).
    fn set_directory_modified(&self, _path: &ResourcePath, _unix_ms: u64) -> Result<()> {
        Err(CoreError::new(ErrorCode::Unsupported, "timestamps unavailable"))
    }
    fn delete_file(&self, path: &ResourcePath) -> Result<()>;
    fn delete_empty_directory(&self, path: &ResourcePath) -> Result<()>;
    fn prepare_move_source(&self, _path: &ResourcePath) -> Result<Box<dyn MoveSource>> {
        Err(CoreError::new(ErrorCode::Unsupported, "storage cannot pin and conditionally remove a source version"))
    }
    fn pin_destination(&self, _path: &ResourcePath) -> Result<Box<dyn PinnedDestination>> {
        Err(CoreError::new(ErrorCode::Unsupported, "storage cannot protect a published destination"))
    }
}

/// Directory listing for display with the number of omitted entries.
#[derive(Clone, Debug)]
pub struct PartialListing { pub entries: Vec<DirectoryEntry>, pub skipped: u64 }

/// IDs are append-only: an existing grant can never be replaced or rebound.
#[derive(Default)]
pub struct StorageRegistry {
    providers: RwLock<HashMap<StorageId, Arc<dyn Storage>>>,
}

impl StorageRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register(&mut self, provider: Arc<dyn Storage>) -> Result<()> {
        self.register_shared(provider)
    }

    pub(crate) fn register_shared(&self, provider: Arc<dyn Storage>) -> Result<()> {
        if provider.domain() != StorageDomain::Files {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "private vault cannot enter the file registry"));
        }
        let id = provider.id();
        let mut providers = self.providers.write().map_err(|_| CoreError::new(ErrorCode::Internal, "storage registry unavailable"))?;
        if providers.contains_key(&id) {
            return Err(CoreError::new(ErrorCode::AlreadyExists, "storage ID already registered"));
        }
        providers.insert(id, provider);
        Ok(())
    }

    /// Remove a provider from the live registry. The journal binding stays, so
    /// the same ID can only come back bound to the same storage.
    pub(crate) fn unregister(&self, id: StorageId) -> Result<()> {
        let mut providers = self.providers.write().map_err(|_| CoreError::new(ErrorCode::Internal, "storage registry unavailable"))?;
        providers.remove(&id).map(|_| ()).ok_or_else(|| CoreError::new(ErrorCode::NotFound, "storage grant unavailable"))
    }

    pub fn get(&self, id: StorageId) -> Result<Arc<dyn Storage>> {
        self.providers.read().map_err(|_| CoreError::new(ErrorCode::Internal, "storage registry unavailable"))?
            .get(&id).cloned().ok_or_else(|| CoreError::new(ErrorCode::NotFound, "storage grant unavailable"))
    }

    pub fn capabilities(&self, id: StorageId) -> Result<Capabilities> {
        Ok(self.get(id)?.capabilities())
    }

    pub(crate) fn bindings(&self) -> Result<Vec<(StorageId, String)>> {
        let providers = self.providers.read().map_err(|_| CoreError::new(ErrorCode::Internal, "storage registry unavailable"))?;
        providers.values().map(|provider| {
            let key = provider.binding_key()?;
            if key.is_empty() {
                return Err(CoreError::new(ErrorCode::InvalidInput, "storage binding cannot be empty"));
            }
            Ok((provider.id(), blake3::hash(key.as_bytes()).to_hex().to_string()))
        }).collect()
    }
}

