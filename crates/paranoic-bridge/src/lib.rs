//! Single UniFFI boundary for Android and Windows. No old engine or JNI ABI.
mod types;
mod android;
mod android_mapped;
mod features;
mod attention;
mod crypto_files;
pub use crypto_files::*;
mod cryptomator;
pub use cryptomator::*;
mod archives;
pub use archives::*;
mod documents;
mod spool;
mod archive_work;
mod archive_io;
mod document_workers;
mod diagnostics;
pub use diagnostics::*;
mod vault_probe;
pub use vault_probe::*;
mod integration;
pub use integration::*;
mod vault_io;
mod cryptomator_io;
mod vault_documents;
pub(crate) use vault_documents::PdfKey;
pub use vault_io::*;
pub use document_workers::*;
mod sync;
pub use sync::*;
pub use documents::*;
pub use attention::*;
pub use features::*;
#[cfg(test)]
mod platform_contract_tests;
#[cfg(test)]
mod features_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod attention_tests;
#[cfg(test)]
mod crypto_files_tests;
#[cfg(test)]
mod cryptomator_tests;
#[cfg(test)]
mod archives_tests;
#[cfg(test)]
mod documents_tests;
#[cfg(test)]
mod saf_vault_tests;
pub use types::*;
use std::{io::Read, path::{Path, PathBuf}, sync::{Arc, Condvar, Mutex}, time::{Duration, Instant}};
use paranoic_core::{Core, CoreConfig, EntryKind, JobId, Location, Metadata, Operation, ReadHandle, ResourcePath, Storage, StorageId, StorageRegistry};
use paranoic_local::{Access, LocalStorage};
use uuid::Uuid;

uniffi::setup_scaffolding!();
/// Bounded image decoding: at most two at once, and a request waits at most
/// a few seconds for a slot, so one stuck provider read cannot block every
/// other thumbnail indefinitely (the stuck read itself is not interrupted).
static IMAGE_BUSY: Mutex<usize> = Mutex::new(0);
static IMAGE_FREE: Condvar = Condvar::new();
const IMAGE_SLOTS: usize = 2;
struct ImageSlot;
impl Drop for ImageSlot {
    fn drop(&mut self) {
        if let Ok(mut busy) = IMAGE_BUSY.lock() { *busy = busy.saturating_sub(1); }
        IMAGE_FREE.notify_one();
    }
}
fn image_slot() -> Result<ImageSlot> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut busy = IMAGE_BUSY.lock().map_err(lock_error)?;
    while *busy >= IMAGE_SLOTS {
        let now = Instant::now();
        if now >= deadline { return Err(BridgeError::Unavailable); }
        busy = IMAGE_FREE.wait_timeout(busy, deadline - now).map_err(lock_error)?.0;
    }
    *busy += 1;
    Ok(ImageSlot)
}
/// Session-scoped encrypted thumbnail cache in a dedicated private directory
/// under the state root. Unavailable cache = thumbnails are decoded each time.
fn thumbnail_cache(state_root: &Path) -> Option<paranoic_documents::ThumbnailCache> {
    let directory = state_root.join("thumbnails");
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
    match builder.create(&directory) {
        Ok(()) => {},
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
        Err(_) => return None,
    }
    let private = paranoic_crypto::PrivateDirectory::open(&directory).ok()?;
    paranoic_documents::ThumbnailCache::ephemeral(private).ok()
}

/// Callbacks are synchronous OS IO, invoked off the Kotlin UI thread. Statuses:
/// 0=ok, 1=permission, 2=unsupported, 3=IO, 4=limit, 5=not found. No exceptions
/// or paths in logs. Paths are '/'-separated display names below the tree root.
#[uniffi::export(with_foreign)]
pub trait AndroidDocuments: Send + Sync {
    fn metadata(&self, tree: String, path: String) -> HostInfo;
    fn list(&self, tree: String, path: String, limit: u32) -> HostListing;
    fn open(&self, tree: String, path: String) -> HostOpened;
    fn read(&self, handle: u64, offset: u64, size: u32) -> HostBytes;
    fn file_metadata(&self, handle: u64) -> HostInfo;
    fn close(&self, handle: u64);
    /// Whether the persisted grant includes write permission.
    fn writable(&self, tree: String) -> bool;
    /// DocumentsContract.createDocument; returns the name the provider actually used.
    fn create(&self, tree: String, parent: String, name: String, directory: bool) -> HostCreated;
    /// Open the document (by URI from create/resolve) for writing ("w").
    fn open_write(&self, tree: String, document: String) -> HostOpened;
    fn write(&self, handle: u64, data: Vec<u8>) -> u32;
    /// Flush, fsync where possible and close a write handle.
    fn finish_write(&self, handle: u64) -> u32;
    /// DocumentsContract.renameDocument of this document; returns the actual
    /// name and the (possibly new) document URI.
    fn rename(&self, tree: String, document: String, name: String) -> HostCreated;
    /// DocumentsContract.deleteDocument of this document (files only; Rust
    /// never deletes SAF directories because that call is recursive).
    fn delete(&self, tree: String, document: String) -> u32;
    /// Resolve a path to its document (verified parent chain).
    fn resolve(&self, tree: String, path: String) -> HostCreated;
    /// E2.8: DocumentsContract.moveDocument of `document` from the folder
    /// `source_parent` into the folder `target_parent` (all document URIs from
    /// resolve/create). Keeps the name; must NOT replace an existing document
    /// of that name (the platform storage provider refuses: error, nothing
    /// moved). Returns the moved document's name and URI. Error 2 if the
    /// provider cannot move.
    fn move_document(&self, tree: String, document: String, source_parent: String, target_parent: String) -> HostCreated;
}

/// Host key bound to the device (Android Keystore AES-GCM, Windows DPAPI)
/// used to seal Core journal records. `available` false = unsealed journal.
/// Status as for AndroidDocuments; output must be authenticated ciphertext.
#[uniffi::export(with_foreign)]
pub trait RecordSealer: Send + Sync {
    fn available(&self) -> bool;
    fn seal(&self, data: Vec<u8>) -> HostBytes;
    fn open(&self, data: Vec<u8>) -> HostBytes;
}
struct JournalKey(Arc<dyn RecordSealer>);
impl paranoic_core::JournalSealer for JournalKey {
    fn seal(&self, plaintext: &[u8]) -> paranoic_core::Result<Vec<u8>> {
        let sealed = self.0.seal(plaintext.to_vec());
        android::status(sealed.error)?; Ok(sealed.data)
    }
    fn open(&self, sealed: &[u8]) -> paranoic_core::Result<Vec<u8>> {
        let plain = self.0.open(sealed.to_vec());
        android::status(plain.error)?; Ok(plain.data)
    }
}

#[derive(uniffi::Object)]
pub struct NativeRuntime {
    core: Core,
    state_root: PathBuf,
    workspace_root: PathBuf,
    pdfium: PathBuf,
    thumbnails: Option<paranoic_documents::ThumbnailCache>,
    pdf_worker: PathBuf,
    /// One live out-of-process PDF session (document parsed once per view).
    pdf: Mutex<Option<(PdfKey, paranoic_documents::pdf_client::PdfSession)>>,
    document_workers: Mutex<Option<Arc<dyn paranoic_documents::worker_process::WorkerHost>>>,
    host: Arc<dyn AndroidDocuments>,
    grants: Mutex<Vec<NativeGrant>>,
    spaces: Mutex<Vec<NativeSpace>>,
    /// Host record key (journal, sealed cloud credentials).
    sealer: Arc<dyn RecordSealer>,
    /// The app's own workspace (also the private spool for vault/cloud work).
    app: Arc<LocalStorage>,
    notes: Mutex<features::NotesSlot>,
    /// Revokes authentication work that began before Quick Lock.
    secrets_epoch: std::sync::atomic::AtomicU64,
    vaults: Mutex<std::collections::HashMap<String, Arc<paranoic_formats::paranoic_vault::ParanoicVault>>>,
    /// Open Cryptomator vaults; forgotten on Quick Lock.
    cryptomator_vaults: Mutex<std::collections::HashMap<String, Arc<paranoic_formats::CryptomatorReader>>>,
    archive_work: archive_work::ArchiveWork,
    /// Connected clouds by storage ID (sync needs the cloud adapter itself).
    clouds: Mutex<std::collections::HashMap<StorageId, Arc<paranoic_cloud::CloudStorage>>>,
}
fn lock_error<T>(_: std::sync::PoisonError<T>) -> BridgeError { BridgeError::Unavailable }
fn root_path(value: String) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() { return Err(BridgeError::Invalid); }
    std::fs::create_dir_all(&path).map_err(|_| BridgeError::Unavailable)?;
    let path = std::fs::canonicalize(path).map_err(|_| BridgeError::Unavailable)?;
    // Windows: app state and workspace are owner-only (user, SYSTEM,
    // Administrators); everything created inside inherits it (register У-1).
    #[cfg(windows)]
    {
        paranoic_windows::ensure_owner_only(&path).map_err(|_| BridgeError::Denied)?;
        if !matches!(paranoic_windows::is_owner_only(&path), Ok(true)) { return Err(BridgeError::Denied); }
    }
    Ok(path)
}
fn grant_id(id: &str) -> Result<StorageId> { Ok(StorageId(Uuid::parse_str(id).map_err(|_| BridgeError::Invalid)?)) }
fn provider(grant: &NativeGrant, host: Arc<dyn AndroidDocuments>, state: &Path, workspace: &Path) -> Result<Arc<dyn Storage>> {
    let id = grant_id(&grant.id)?;
    if grant.android_tree {
        if !cfg!(target_os = "android") || !grant.reference.starts_with("content://") || grant.reference.len() > 8192 {
            return Err(BridgeError::Invalid);
        }
        // Writes only where the provider's no-replace behaviour is known
        // (AOSP ExternalStorageProvider picks a unique name instead of
        // overwriting); every other provider is read-only.
        let writable = host.writable(grant.reference.clone()) && android::trusted_writer(&grant.reference);
        let provider = android::SafStorage { id, tree: grant.reference.clone(), host, writable };
        provider.metadata(&ResourcePath::root())?;
        Ok(Arc::new(provider))
    } else {
        let path = std::fs::canonicalize(&grant.reference).map_err(|_| BridgeError::Denied)?;
        if path.starts_with(state) || state.starts_with(&path) || path.starts_with(workspace) || workspace.starts_with(&path) {
            return Err(BridgeError::Denied);
        }
        // Writable: Linux stages in 0700 folders, Windows in owner-only ACL folders
        // (register У-19). The OS still decides whether the user may write there.
        // Android: a non-SAF grant is shared storage reached through "All
        // files access" (FUSE, no renameat2 flags, no hard links).
        if cfg!(target_os = "android") {
            return Ok(Arc::new(LocalStorage::open_shared(id, path, Access::ReadWrite)?));
        }
        Ok(Arc::new(LocalStorage::open(id, path, Access::ReadWrite)?))
    }
}
fn space(provider: &dyn Storage, label: String) -> NativeSpace {
    let id = provider.id().0.to_string();
    NativeSpace { id: id.clone(), name: label, root: NativeLocation { storage_id: id, path: String::new() },
        writable: provider.capabilities().write, detachable: true }
}
#[uniffi::export]
impl NativeRuntime {
    #[uniffi::constructor]
    pub fn new(state_root: String, workspace_root: String, pdfium_library: String, pdf_worker: String,
        saved_grants: Vec<NativeGrant>, documents: Arc<dyn AndroidDocuments>, sealer: Arc<dyn RecordSealer>) -> Result<Arc<Self>> {
        if saved_grants.len() > 128 { return Err(BridgeError::Limit); }
        let state_root = root_path(state_root)?;
        let workspace_root = root_path(workspace_root)?;
        if state_root.starts_with(&workspace_root) || workspace_root.starts_with(&state_root) { return Err(BridgeError::Denied); }
        let mut registry = StorageRegistry::new();
        let app_id = StorageId(Uuid::from_u128(0x706172616e6f69630000000000000001));
        let app = Arc::new(LocalStorage::open_private(app_id, &workspace_root)?);
        let mut spaces = vec![NativeSpace { detachable: false, ..space(app.as_ref(), "Paranoic".into()) }];
        registry.register(app.clone())?;
        for grant in &saved_grants {
            // Cloud grants connect over the network: reconnect_cloud() after start.
            if features::is_cloud(grant) { continue; }
            // Revoked grants are retained in the protected host store but not
            // presented as available. Re-attachment uses the same stored UUID.
            match provider(grant, Arc::clone(&documents), &state_root, &workspace_root) {
                Ok(p) => { spaces.push(space(p.as_ref(), label(grant))); registry.register(p)?; }
                Err(BridgeError::Denied | BridgeError::Unavailable) => {},
                Err(e) => return Err(e),
            }
        }
        let journal_key: Option<Arc<dyn paranoic_core::JournalSealer>> =
            if sealer.available() { Some(Arc::new(JournalKey(Arc::clone(&sealer)))) } else { None };
        let core = Core::open_sealed(&state_root, CoreConfig::default(), registry, journal_key)?;
        features::sweep_spools_at_start(app.as_ref(), &core);
        let thumbnails = thumbnail_cache(&state_root);
        Ok(Arc::new(Self { core, thumbnails, state_root, workspace_root, pdfium: PathBuf::from(pdfium_library),
            pdf_worker: PathBuf::from(pdf_worker), pdf: Mutex::new(None), document_workers: Mutex::new(None),
            host: documents, grants: Mutex::new(saved_grants), spaces: Mutex::new(spaces), sealer, app,
            notes: Mutex::new(features::NotesSlot::default()), secrets_epoch: std::sync::atomic::AtomicU64::new(0),
            vaults: Mutex::new(Default::default()),
            cryptomator_vaults: Mutex::new(Default::default()), archive_work: archive_work::ArchiveWork::default(),
            clouds: Mutex::new(Default::default()) }))
    }
    pub fn spaces(&self) -> Result<Vec<NativeSpace>> { Ok(self.spaces.lock().map_err(lock_error)?.clone()) }
    pub fn grants(&self) -> Result<Vec<NativeGrant>> { Ok(self.grants.lock().map_err(lock_error)?.clone()) }
    /// Diagnostics: does this place's filesystem honour RENAME_NOREPLACE?
    /// 0 supported, 1 rejected (fallback in use), 2 IGNORED (replaces!), 3 probe failed,
    /// 4 not a local folder (SAF tree, cloud) or unknown place.
    pub fn rename_probe(&self, storage_id: String) -> Result<u8> {
        let grant = self.grants.lock().map_err(lock_error)?.iter().find(|g| g.id == storage_id).cloned();
        let Some(grant) = grant else { return Ok(4) };
        if grant.android_tree || features::is_cloud(&grant) { return Ok(4); }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            use paranoic_local::NoReplaceProbe as P;
            // The probe opens the root once and works relative to that descriptor.
            return Ok(match paranoic_local::probe_noreplace(std::path::Path::new(&grant.reference)) { P::Supported => 0, P::Unsupported => 1, P::Ignored => 2, P::Failed => 3 });
        }
        #[allow(unreachable_code)]
        Ok(4)
    }
    pub fn attach(&self, grant: NativeGrant) -> Result<NativeSpace> {
        let mut grants = self.grants.lock().map_err(lock_error)?;
        // An already connected folder is returned before the limit applies.
        if let Some(existing) = grants.iter().find(|g| g.id == grant.id || (g.reference == grant.reference && g.android_tree == grant.android_tree)) {
            if let Some(space) = self.spaces.lock().map_err(lock_error)?.iter().find(|s| s.id == existing.id) { return Ok(space.clone()); }
            if existing.id != grant.id { return Err(BridgeError::Conflict); }
        }
        if grants.len() >= 128 && !grants.iter().any(|g| g.id == grant.id) { return Err(BridgeError::Limit); }
        let p = provider(&grant, Arc::clone(&self.host), &self.state_root, &self.workspace_root)?;
        let value = space(p.as_ref(), label(&grant));
        self.core.register_storage(p)?;
        grants.retain(|g| g.id != grant.id);
        grants.push(grant);
        self.spaces.lock().map_err(lock_error)?.push(value.clone());
        Ok(value)
    }
    /// Disconnect a folder: Core refuses while a queued or running job uses
    /// it. The journal binding stays, so unresolved jobs resume only if the
    /// same folder is connected again under the same ID. Returns the removed
    /// grant so the host can release its OS permission and update its store.
    pub fn detach(&self, id: String) -> Result<NativeGrant> {
        let storage = grant_id(&id)?;
        let mut grants = self.grants.lock().map_err(lock_error)?;
        let index = grants.iter().position(|g| g.id == id).ok_or(BridgeError::Invalid)?;
        match self.core.unregister_storage(storage) {
            Ok(()) => {},
            // Revoked at startup: never registered, nothing to withdraw.
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound => {},
            Err(error) => return Err(error.into()),
        }
        self.spaces.lock().map_err(lock_error)?.retain(|s| s.id != id);
        self.clouds.lock().map_err(lock_error)?.remove(&storage);
        Ok(grants.remove(index))
    }
    /// Journal lookup for a request whose submission reply was lost. `None`
    /// means the request never reached the journal (safe to submit again with
    /// the same request ID; resubmission with the same ID never repeats a job).
    pub fn job(&self, id: String) -> Result<Option<NativeJob>> {
        match self.core.job(JobId(grant_id(&id)?.0)) {
            Ok(job) => Ok(Some(job.into())),
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    /// Display listing: entries that cannot be represented (non-portable or
    /// duplicate names) are omitted and counted in `skipped`, never fatal.
    pub fn list(&self, location: NativeLocation) -> Result<NativeListing> {
        let listing = self.core.list_for_display(&location.core()?)?;
        let app = self.app.id();
        let unknown: std::collections::HashSet<String> = listing.unknown_size.iter().map(|p| p.as_str().to_owned()).collect();
        let entries = listing.entries.into_iter().map(|e| {
            let name = e.location.path.parent_and_name()?.1.to_owned();
            // Service objects of the app's own folder: spools and the personal folder (plan 5.1).
            if e.location.storage == app && (features::is_spool(&name) || e.location.path.as_str() == features::SAFE_DIR) { return Ok(None); }
            let size_known = !unknown.contains(e.location.path.as_str());
            Ok(Some(NativeEntry { location: e.location.into(), name, kind: types::kind(e.metadata.kind), bytes: e.metadata.bytes,
                modified_ms: e.metadata.modified_unix_ms, size_known }))
        }).collect::<Result<Vec<_>>>()?.into_iter().flatten().collect();
        Ok(NativeListing { entries, skipped: listing.skipped })
    }
    pub fn create_directory(&self, parent: NativeLocation, name: String, request_id: String) -> Result<NativeJob> {
        let parent = parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), Operation::CreateDirectory { target })?.into())
    }
    pub fn copy(&self, source: NativeLocation, parent: NativeLocation, name: String, request_id: String) -> Result<NativeJob> {
        let source = source.core()?;
        let parent = parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let operation = match self.core.metadata(&source)?.kind {
            EntryKind::File => Operation::CopyFile { source, target, conflict: paranoic_core::ConflictPolicy::Fail },
            EntryKind::Directory => Operation::CopyDirectory { source, target, conflict: paranoic_core::ConflictPolicy::Fail },
            _ => return Err(BridgeError::Unsupported),
        };
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), operation)?.into())
    }
    /// Copy (`mode` 0) or move (`mode` 1) a file or folder into `parent` as `name`.
    /// `conflict` (copy only): 0 refuse, 1 skip, 2 keep both ("name (N).ext"),
    /// 3 replace — file over file only, the old file is kept until the new one
    /// is published. A move never replaces: only 0 is accepted. Same storage:
    /// atomic rename without replace; across storages: copy, verify, then remove
    /// the source (folders: unchanged entries only, leftovers reported).
    pub fn transfer(&self, source: NativeLocation, parent: NativeLocation, name: String, mode: u8, conflict: u8,
        request_id: String) -> Result<NativeJob> {
        let source = source.core()?;
        let parent = parent.core()?;
        if source.path.is_root() { return Err(BridgeError::Invalid); }
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let policy = match conflict {
            0 => paranoic_core::ConflictPolicy::Fail, 1 => paranoic_core::ConflictPolicy::Skip,
            2 => paranoic_core::ConflictPolicy::KeepBoth, 3 => paranoic_core::ConflictPolicy::Replace,
            _ => return Err(BridgeError::Invalid),
        };
        let kind = self.core.metadata(&source)?.kind;
        let operation = match (mode, kind) {
            (0, EntryKind::File) => Operation::CopyFile { source, target, conflict: policy },
            (0, EntryKind::Directory) => Operation::CopyDirectory { source, target, conflict: policy },
            (1, EntryKind::File | EntryKind::Directory) if conflict != 0 => return Err(BridgeError::Invalid),
            (1, _) if target.path.as_str().starts_with(&format!("{}/", source.path.as_str())) && source.storage == target.storage =>
                return Err(BridgeError::Invalid), // into itself
            (1, EntryKind::File | EntryKind::Directory) if source.storage == target.storage => Operation::Move { source, target },
            (1, EntryKind::File) => Operation::MoveFileAcross { source, target },
            (1, EntryKind::Directory) => Operation::MoveDirectoryAcross { source, target },
            (0 | 1, _) => return Err(BridgeError::Unsupported),
            _ => return Err(BridgeError::Invalid),
        };
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), operation)?.into())
    }
    /// Rename in place (atomic, never replaces an existing name).
    pub fn rename(&self, location: NativeLocation, new_name: String, request_id: String) -> Result<NativeJob> {
        let source = location.core()?;
        let (parent, _) = source.path.parent_and_name()?;
        let target = Location { storage: source.storage, path: parent.join(&new_name)? };
        if target.path == source.path { return Err(BridgeError::Invalid); }
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), Operation::Move { source, target })?.into())
    }
    /// Delete a file, an empty folder, or (`recursive`) a folder with content.
    /// Cloud storages move to the provider trash; local storages delete. The
    /// root of a space is never deleted. Storages without delete capability
    /// (e.g. folders of an Android document tree) refuse.
    pub fn delete(&self, location: NativeLocation, recursive: bool, request_id: String) -> Result<NativeJob> {
        let target = location.core()?;
        if target.path.is_root() { return Err(BridgeError::Invalid); }
        let operation = match self.core.metadata(&target)?.kind {
            EntryKind::File => Operation::DeleteFile { target },
            EntryKind::Directory if recursive => Operation::DeleteDirectory { target },
            EntryKind::Directory => Operation::DeleteEmptyDirectory { target },
            _ => return Err(BridgeError::Unsupported),
        };
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), operation)?.into())
    }
    pub fn jobs(&self) -> Result<Vec<NativeJob>> { Ok(self.core.recent_jobs(128)?.into_iter().map(Into::into).collect()) }
    pub fn cancel(&self, id: String) -> Result<NativeJob> { Ok(self.core.cancel(JobId(grant_id(&id)?.0))?.into()) }
    pub fn open_stream(&self, location: NativeLocation) -> Result<Arc<NativeReader>> {
        let reader = self.core.open_read(&location.core()?)?;
        let metadata = reader.metadata()?;
        if metadata.kind != EntryKind::File { return Err(BridgeError::Unsupported); }
        Ok(Arc::new(NativeReader { info: metadata, state: Mutex::new(Some((reader, 0))) }))
    }
    pub fn inspect(&self, location: NativeLocation) -> Result<NativePreview> {
        let reader = self.open_stream(location)?;
        let header = reader.read_at(0, 64)?;
        let (kind, mime) = if header.starts_with(b"%PDF-") { (1, "application/pdf") }
            else if let Some(media) = paranoic_documents::sniff(&header) {
                (match media.kind { paranoic_documents::MediaKind::Image => 2, paranoic_documents::MediaKind::Audio => 3, paranoic_documents::MediaKind::Video => 4 }, media.mime)
            } else { (0, "application/octet-stream") };
        Ok(NativePreview { kind, mime: mime.into(), length: reader.info.bytes })
    }
    pub fn image_png(&self, location: NativeLocation, side: u32) -> Result<Vec<u8>> {
        let _slot = image_slot()?;
        // 256 MiB of decoded RGBA ≈ 64 MP: full-size photos of 50 MP phone cameras (64 MiB stopped at ~16 MP).
        let limits = paranoic_documents::ImageLimits { input_bytes: 32 << 20, max_width: 16384, max_height: 16384, max_alloc: 256 << 20 };
        // Cache key includes length AND modification time: a replaced file of
        // the same size gets a new thumbnail.
        // Only list thumbnails use the encrypted cache. Full-screen previews (PNGs often over its
        // 4 MiB entry bound) are decoded directly.
        if let Some(cache) = self.thumbnails.as_ref().filter(|_| side <= 512) {
            return Ok(paranoic_documents::cached_thumbnail(&self.core, cache, &location.core()?, side, limits, || Ok(()))?.to_vec());
        }
        let reader = self.open_stream(location)?;
        if reader.info.bytes > 32 << 20 { return Err(BridgeError::Limit); }
        let mut data = zeroize::Zeroizing::new(Vec::new());
        while (data.len() as u64) < reader.info.bytes {
            let part = reader.read_at(data.len() as u64, 1024 * 1024)?;
            if part.is_empty() { return Err(BridgeError::Changed); }
            data.extend_from_slice(&part);
        }
        Ok(paranoic_documents::thumbnail_image(&data, side, limits)?.to_vec())
    }
    pub fn pdf_page(&self, location: NativeLocation, page: u32, width: u32) -> Result<NativePdfPage> {
        self.with_pdf(location, |session| {
            let png = session.render(page, width)?;
            Ok(NativePdfPage { pages: session.pages(), index: page, png })
        })
    }
    pub fn pdf_search(&self, location: NativeLocation, query: String, start: u32) -> Result<NativePdfMatches> {
        self.with_pdf(location, |session| {
            let (pages, next_page) = session.search(&query, start)?;
            Ok(NativePdfMatches { pages, next_page })
        })
    }
    /// Close the current PDF session (viewer closed, Quick Lock).
    pub fn close_pdf(&self) -> Result<()> { self.pdf.lock().map_err(lock_error)?.take(); Ok(()) }
    pub fn shutdown(&self) -> Result<()> {
        self.archive_work.cancel_all();
        if let Ok(mut pdf) = self.pdf.lock() { pdf.take(); }
        if let Some(cache) = &self.thumbnails { let _ = cache.lock(); }
        self.lock_secrets();
        self.core.request_shutdown()?; Ok(())
    }
}
impl NativeRuntime {
    /// Reuse the worker only for the same, unchanged file; otherwise restart it.
    fn with_pdf<T>(&self, location: NativeLocation, f: impl FnOnce(&mut paranoic_documents::pdf_client::PdfSession)
        -> paranoic_core::Result<T>) -> Result<T> {
        let location = location.core()?;
        let current = self.core.metadata(&location)?;
        let mut slot = self.pdf.lock().map_err(lock_error)?;
        let key = PdfKey::Plain(location.clone());
        let reusable = slot.as_ref().is_some_and(|(k, s)| *k == key && s.is_alive() && *s.source() == current);
        if !reusable {
            slot.take();
            let session = paranoic_documents::pdf_client::PdfSession::open_with_host(&self.core, &location,
                &self.pdf_worker, &self.pdfium, self.document_worker_host()?)?;
            *slot = Some((key, session));
        }
        let (_, session) = slot.as_mut().ok_or(BridgeError::Unavailable)?;
        Ok(f(session)?)
    }
}
fn label(grant: &NativeGrant) -> String {
    if grant.android_tree { "Android SAF".into() } else {
        Path::new(&grant.reference).file_name().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "Files".into())
    }
}
type ReaderState = Option<(Box<dyn ReadHandle>, u64)>;
#[derive(uniffi::Object)]
pub struct NativeReader { info: Metadata, state: Mutex<ReaderState> }
#[uniffi::export]
impl NativeReader {
    pub fn length(&self) -> u64 { self.info.bytes }
    pub fn read_at(&self, offset: u64, size: u32) -> Result<Vec<u8>> {
        if size > 1024 * 1024 { return Err(BridgeError::Limit); }
        let mut state = self.state.lock().map_err(lock_error)?;
        let (reader, position) = state.as_mut().ok_or(BridgeError::Denied)?;
        if reader.metadata()? != self.info { return Err(BridgeError::Changed); }
        if offset >= self.info.bytes || size == 0 { return Ok(Vec::new()); }
        if *position != offset { reader.seek_to(offset)?; *position = offset; }
        let wanted = (self.info.bytes - offset).min(size as u64) as usize;
        let mut data = vec![0; wanted];
        let mut filled = 0;
        while filled < wanted {
            let n = reader.read(&mut data[filled..]).map_err(paranoic_core::CoreError::io)?;
            if n == 0 { return Err(BridgeError::Changed); }
            filled += n;
            *position += n as u64;
        }
        if reader.metadata()? != self.info { return Err(BridgeError::Changed); }
        Ok(data)
    }
    /// Separate from UniFFI's generated AutoCloseable.close()/destroy().
    pub fn release_stream(&self) { self.close(); }
}
impl NativeReader {
    /// Rust callers retain the original API; do not export a second Kotlin close().
    pub fn close(&self) { if let Ok(mut state) = self.state.lock() { state.take(); } }
}

