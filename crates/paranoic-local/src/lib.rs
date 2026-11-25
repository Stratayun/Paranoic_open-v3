//! Capability-confined native filesystem adapter.
//!
//! Only open() uses ambient authority, at an explicit host permission boundary.
//! All subsequent operations resolve through directory handles. Android SAF is
//! a distinct future adapter; this is for actual native filesystem grants.

#[cfg(any(target_os = "linux", target_os = "android"))]
mod bootstrap;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod mapped_rename;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod directory_reshape;

#[cfg(unix)]
mod shared;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use shared::{probe_noreplace, NoReplaceProbe};
#[cfg(windows)]
mod windows;

use std::{io::{Read, Write}, path::Path, time::UNIX_EPOCH};

use cap_std::{ambient_authority, fs::{Dir, DirBuilder, File, OpenOptions}};
use paranoic_core::{Capabilities, CoreError, DirectoryEntry, EntryKind, ErrorCode, JobId,
    Location, Metadata, ReadHandle, ResourcePath, Result, StagedWrite, Storage, StorageDomain, StorageId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access { ReadOnly, ReadWrite }

pub struct LocalStorage {
    id: StorageId,
    root: Dir,
    access: Access,
    private_root: bool,
    /// Android shared storage (FUSE): see `shared.rs`.
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    shared_root: bool,
    /// Shared storage only: whether this volume really refuses to replace on
    /// RENAME_NOREPLACE, measured once by [`probe_noreplace`] (audit 1, 2).
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    shared_noreplace: std::sync::OnceLock<bool>,
    /// Absolute root path, needed only by Windows calls that take paths
    /// (ACL of staging folders, no-replace rename).
    #[cfg_attr(not(windows), allow(dead_code))]
    root_path: std::path::PathBuf,
}

impl LocalStorage {
    /// The host must persist `id` with the grant and never reassign it to a
    /// different root. Never grant a parent of the core state or Private Vault.
    pub fn open(id: StorageId, root: impl AsRef<Path>, access: Access) -> Result<Self> {
        let root_path = std::fs::canonicalize(root.as_ref()).map_err(CoreError::io)?;
        let root = Dir::open_ambient_dir(&root_path, ambient_authority()).map_err(CoreError::io)?;
        Ok(Self { id, root, access, private_root: false, shared_root: false, shared_noreplace: std::sync::OnceLock::new(), root_path })
    }

    /// Android shared storage reached through "All files access"
    /// (`/storage/emulated/N`, SD cards). Publication there is offered only
    /// when the volume honours RENAME_NOREPLACE (checked once, see
    /// `shared.rs`); otherwise writes are refused before any change.
    pub fn open_shared(id: StorageId, root: impl AsRef<Path>, access: Access) -> Result<Self> {
        let mut storage = Self::open(id, root, access)?;
        storage.shared_root = true;
        Ok(storage)
    }

    /// Only for the host-created, per-user application files directory, whose
    /// ACL excludes other users. Never use for an arbitrary user-selected root.
    pub fn open_private(id: StorageId, root: impl AsRef<Path>) -> Result<Self> {
        let mut storage = Self::open(id, root, Access::ReadWrite)?;
        storage.private_root = true;
        Ok(storage)
    }

    /// OS path of a resource below the root (Windows path-based calls only).
    #[cfg(windows)]
    pub(crate) fn os_path(&self, path: &ResourcePath) -> std::path::PathBuf {
        let mut out = self.root_path.clone();
        if !path.is_root() { for part in path.as_str().split('/') { out.push(part); } }
        out
    }

    /// Shared storage: true when the volume honours RENAME_NOREPLACE. Any other
    /// verdict (rejected, ignored, probe failed) means no publication there.
    fn shared_noreplace(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if self.shared_root {
            return *self.shared_noreplace.get_or_init(|| self.access == Access::ReadWrite
                && crate::shared::probe_noreplace(&self.root_path) == crate::shared::NoReplaceProbe::Supported);
        }
        true
    }
    /// Operations that publish by rename need a real no-replace rename.
    fn require_noreplace(&self) -> Result<()> {
        if self.shared_noreplace() { Ok(()) } else {
            Err(CoreError::new(ErrorCode::Unsupported, "this shared storage cannot rename without replacing (RENAME_NOREPLACE unavailable)"))
        }
    }

    fn writable(&self) -> Result<()> {
        // Windows: staging folders are made owner-only (ACL) and verified, so
        // user-selected folders are writable too (register У-19).
        if !cfg!(unix) && !cfg!(windows) {
            return Err(CoreError::new(ErrorCode::Unsupported, "native writes require a platform-specific secure staging adapter"));
        }
        if self.access == Access::ReadWrite { Ok(()) }
        else { Err(CoreError::new(ErrorCode::PermissionDenied, "storage grant is read-only")) }
    }

    fn directory(&self, path: &ResourcePath) -> Result<Dir> {
        #[cfg(unix)]
        {
            // Resolve one component at a time through held handles. A raced
            // symlink cannot redirect a recursive mutation into another tree.
            let mut directory = self.root.try_clone().map_err(CoreError::io)?;
            if path.is_root() { return Ok(directory); }
            for component in path.as_str().split('/') {
                let fd = rustix::fs::openat(&directory, component,
                    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty())
                    .map_err(|error| CoreError::io(std::io::Error::from(error)))?;
                directory = Dir::from_std_file(std::fs::File::from(fd));
            }
            Ok(directory)
        }
        #[cfg(not(unix))]
        {
            if path.is_root() { self.root.try_clone().map_err(CoreError::io) }
            else { self.root.open_dir(path.as_str()).map_err(CoreError::io) }
        }
    }

    fn parent<'a>(&self, path: &'a ResourcePath) -> Result<(Dir, &'a str)> {
        let (parent, name) = path.parent_and_name()?;
        Ok((self.directory(&parent)?, name))
    }
}

impl Storage for LocalStorage {
    fn id(&self) -> StorageId { self.id }

    fn binding_key(&self) -> Result<String> {
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            let metadata = self.root.dir_metadata().map_err(CoreError::io)?;
            Ok(format!("native-unix:{}:{}", metadata.dev(), metadata.ino()))
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsHandle;
            // 128-bit FileId (NTFS, ReFS) plus the volume serial (register У-3).
            // Volumes without stable ids report zero: fail closed.
            let (volume, id) = paranoic_windows::file_id(self.root.as_handle()).map_err(CoreError::io)?;
            if volume == 0 || id == [0u8; 16] {
                return Err(CoreError::new(ErrorCode::Unsupported, "volume provides no stable folder identity"));
            }
            let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
            Ok(format!("native-windows-fileid:{volume:016x}:{hex}"))
        }
        #[cfg(not(any(unix, windows)))]
        {
            // Stable file-ID binding is required before Windows can resume
            // persisted jobs. Do not substitute a path, which can be rebound.
            Err(CoreError::new(ErrorCode::Unsupported, "stable native root identity is not implemented on this platform"))
        }
    }

    fn domain(&self) -> StorageDomain { StorageDomain::Files }

    fn capabilities(&self) -> Capabilities {
        let writable = self.access == Access::ReadWrite && (cfg!(unix) || cfg!(windows));
        Capabilities {
            read: true, write: writable, delete: writable,
            atomic_move_no_replace: writable && cfg!(any(target_os = "linux", target_os = "android", windows)) && self.shared_noreplace(),
            durable_directory_sync: cfg!(unix),
            // Check-then-act leases (see MoveLease/PinnedFile): the object is
            // identified by dev/ino and ctime, verified immediately before unlink.
            conditional_move_source: writable && cfg!(unix),
            pinned_destination: writable && cfg!(unix),
            directory_sync_waived: false,
        }
    }

    fn metadata(&self, path: &ResourcePath) -> Result<Metadata> {
        if path.is_root() {
            return self.root.dir_metadata().map(convert_metadata).map_err(CoreError::io);
        }
        let (parent, name) = self.parent(path)?;
        parent.symlink_metadata(name).map(convert_metadata).map_err(CoreError::io)
    }

    fn list(&self, path: &ResourcePath, limit: usize) -> Result<Vec<DirectoryEntry>> {
        if !(1..=100_000).contains(&limit) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid directory listing limit"));
        }
        let directory = self.directory(path)?;
        let mut result = Vec::new();
        for (index, entry) in directory.entries().map_err(CoreError::io)?.enumerate() {
            if index >= limit {
                return Err(CoreError::new(ErrorCode::LimitExceeded, "directory exceeds listing limit"));
            }
            let entry = entry.map_err(CoreError::io)?;
            let name = entry.file_name().into_string()
                .map_err(|_| CoreError::new(ErrorCode::Unsupported, "directory contains a non-UTF-8 name"))?;
            if name.to_ascii_lowercase().starts_with(".paranoic-") { continue; }
            let location = Location { storage: self.id, path: path.join(&name)? };
            // Never follow final symlinks just to display a directory entry.
            let metadata = directory.symlink_metadata(&name).map(convert_metadata).map_err(CoreError::io)?;
            result.push(DirectoryEntry { location, metadata });
        }
        result.sort_by(|a, b| a.location.path.as_str().cmp(b.location.path.as_str()));
        Ok(result)
    }

    fn list_for_display(&self, path: &ResourcePath, limit: usize) -> Result<paranoic_core::PartialListing> {
        if !(1..=100_000).contains(&limit) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid directory listing limit"));
        }
        let directory = self.directory(path)?;
        let (mut entries, mut skipped) = (Vec::new(), 0u64);
        for (index, entry) in directory.entries().map_err(CoreError::io)?.enumerate() {
            if index >= limit {
                return Err(CoreError::new(ErrorCode::LimitExceeded, "directory exceeds listing limit"));
            }
            let entry = entry.map_err(CoreError::io)?;
            let Ok(name) = entry.file_name().into_string() else { skipped += 1; continue };
            if name.to_ascii_lowercase().starts_with(".paranoic-") { continue; }
            let Ok(child) = path.join(&name) else { skipped += 1; continue };
            let metadata = directory.symlink_metadata(&name).map(convert_metadata).map_err(CoreError::io)?;
            entries.push(DirectoryEntry { location: Location { storage: self.id, path: child }, metadata });
        }
        entries.sort_by(|a, b| a.location.path.as_str().cmp(b.location.path.as_str()));
        Ok(paranoic_core::PartialListing { entries, skipped, unknown_size: Vec::new() })
    }

    fn open_read(&self, path: &ResourcePath) -> Result<Box<dyn ReadHandle>> {
        let (parent, name) = self.parent(path)?;
        require_kind(&parent, name, EntryKind::File)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            // Prevent a raced FIFO from blocking the worker or a final symlink
            // from changing the requested object. Intermediate paths remain
            // capability-confined by cap-std.
            options.custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
        }
        let file = parent.open_with(name, &options).map_err(CoreError::io)?;
        if !file.metadata().map_err(CoreError::io)?.is_file() {
            return Err(CoreError::new(ErrorCode::Unsupported, "source is not a regular file"));
        }
        Ok(Box::new(LocalReader(file)))
    }

    fn stage(&self, target: &ResourcePath, job: JobId) -> Result<Box<dyn StagedWrite>> {
        self.writable()?;
        self.require_noreplace()?;
        #[cfg(windows)]
        return windows::stage(self, target, job);
        #[cfg(not(windows))]
        {
        let (parent, name) = self.parent(target)?;
        // A private directory, rather than a loose temporary file, prevents
        // other OS users with parent write access from replacing the payload.
        // Recovery derives this artifact name from the durable job ID.
        let temporary = format!(".paranoic-{}", job.0);
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        parent.create_dir_with(&temporary, &builder).map_err(CoreError::io)?;
        let staging = parent.open_dir(&temporary).map_err(CoreError::io)?;
        #[cfg(unix)]
        {
            let metadata = staging.dir_metadata().map_err(CoreError::io)?;
            if !crate::shared::staging_private(&metadata, self.shared_root) {
                return Err(CoreError::new(ErrorCode::PermissionDenied, "staging directory is not private to the current user"));
            }
        }
        let mut stage = LocalStage {
            parent, staging, temporary, target: name.to_owned(), file: None, cleaned: false, shared: self.shared_root,
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        stage.file = Some(stage.staging.open_with("payload", &options).map_err(CoreError::io)?);
        Ok(Box::new(stage))
        }
    }

    fn create_directory(&self, path: &ResourcePath) -> Result<()> {
        self.writable()?;
        let (parent, name) = self.parent(path)?;
        parent.create_dir(name).map_err(CoreError::io)?;
        sync_directory(&parent)
    }

    fn stage_bundle(&self, target: &ResourcePath, job: JobId, content: &str,
        sidecar: &str, data: &[u8]) -> Result<Box<dyn StagedWrite>> {
        self.writable()?;
        self.require_noreplace()?;
        if !cfg!(any(target_os = "linux", target_os = "android")) {
            return Err(CoreError::new(ErrorCode::Unsupported, "atomic bundles unavailable"));
        }
        ResourcePath::root().join(content)?;
        ResourcePath::root().join(sidecar)?;
        if content == sidecar || data.len() > 4096 {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid bundle layout"));
        }
        let (parent, name) = self.parent(target)?;
        let temporary = format!(".paranoic-{}", job.0);
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        { use cap_std::fs::DirBuilderExt; builder.mode(0o700); }
        parent.create_dir_with(&temporary, &builder).map_err(CoreError::io)?;
        #[cfg(unix)]
        let directory = {
            let fd = rustix::fs::openat(&parent, temporary.as_str(),
                rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY |
                rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty())
                .map_err(|e| CoreError::io(std::io::Error::from(e)))?;
            Dir::from_std_file(std::fs::File::from(fd))
        };
        #[cfg(not(unix))]
        let directory = parent.open_dir(&temporary).map_err(CoreError::io)?;
        #[cfg(unix)]
        {
            let metadata = directory.dir_metadata().map_err(CoreError::io)?;
            if !crate::shared::staging_private(&metadata, self.shared_root) {
                return Err(CoreError::new(ErrorCode::PermissionDenied, "bundle staging is not private"));
            }
        }
        let mut stage = LocalBundle { parent, directory, temporary, target: name.to_owned(),
            content: content.to_owned(), sidecar: sidecar.to_owned(), file: None, finished: false, shared: self.shared_root };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        { use cap_std::fs::OpenOptionsExt; options.mode(0o600); }
        let mut mapping = stage.directory.open_with(sidecar, &options).map_err(CoreError::io)?;
        mapping.write_all(data).map_err(CoreError::io)?;
        mapping.sync_all().map_err(CoreError::io)?;
        stage.file = Some(stage.directory.open_with(content, &options).map_err(CoreError::io)?);
        Ok(Box::new(stage))
    }

    fn publish_tree(&self, target: &ResourcePath, job: JobId,
        files: &[paranoic_core::PreparedTreeFile], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { bootstrap::publish(self, target, job, files, checkpoint) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let _ = (target, job, files, checkpoint);
            Err(CoreError::new(ErrorCode::Unsupported, "atomic bootstrap publication unavailable"))
        }
    }

    fn move_no_replace(&self, source: &ResourcePath, target: &ResourcePath) -> Result<()> {
        self.writable()?;
        self.require_noreplace()?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let (source_parent, source_name) = self.parent(source)?;
            let (target_parent, target_name) = self.parent(target)?;
            let metadata = source_parent.symlink_metadata(source_name).map_err(CoreError::io)?;
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(CoreError::new(ErrorCode::Unsupported, "only files and directories can be moved"));
            }
            crate::shared::rename_noreplace(&source_parent, source_name, &target_parent, target_name, self.shared_root)
                .map_err(|error| CoreError::io(std::io::Error::from(error)))?;
            sync_directory(&target_parent)?;
            sync_directory(&source_parent)
        }
        #[cfg(windows)]
        {
            let (source_parent, source_name) = self.parent(source)?;
            self.parent(target)?; // target parent must exist below the root
            let metadata = source_parent.symlink_metadata(source_name).map_err(CoreError::io)?;
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(CoreError::new(ErrorCode::Unsupported, "only files and directories can be moved"));
            }
            paranoic_windows::rename_no_replace(&self.os_path(source), &self.os_path(target)).map_err(CoreError::io)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        {
            let _ = (source, target);
            Err(CoreError::new(ErrorCode::Unsupported, "atomic move without replacement is not implemented on this platform"))
        }
    }

    fn rename_mapped(&self, source: &ResourcePath, target: &ResourcePath, job: JobId,
        old: &[u8], new: &[u8]) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { mapped_rename::rename(self, source, target, job, old, new) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, job, old, new);
          Err(CoreError::new(ErrorCode::Unsupported, "mapped rename unavailable")) }
    }

    fn inspect_mapped_rename(&self, source: &ResourcePath, target: &ResourcePath, job: JobId,
        old: &[u8], new: &[u8], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { mapped_rename::inspect(self, source, target, job, old, new, checkpoint) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, job, old, new, checkpoint);
          Err(CoreError::new(ErrorCode::Unsupported, "mapped rename inspection unavailable")) }
    }

    fn recover_mapped(&self, source: &ResourcePath, target: &ResourcePath,
        original: JobId, recovery: JobId, old: &[u8], new: &[u8], action: paranoic_core::MappedRecoveryAction) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { mapped_rename::recover(self, source, target, original, recovery, old, new, action) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, original, recovery, old, new, action);
          Err(CoreError::new(ErrorCode::Unsupported, "mapped recovery unavailable")) }
    }

    fn reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId,
        layout: &paranoic_core::DirectoryReshape) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { directory_reshape::rename(self, source, target, job, layout) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, job, layout);
          Err(CoreError::new(ErrorCode::Unsupported, "directory reshape unavailable")) }
    }

    fn inspect_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId,
        layout: &paranoic_core::DirectoryReshape, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { directory_reshape::inspect(self, source, target, job, layout, checkpoint) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, job, layout, checkpoint);
          Err(CoreError::new(ErrorCode::Unsupported, "directory reshape inspection unavailable")) }
    }

    fn recover_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath,
        original: JobId, recovery: JobId, layout: &paranoic_core::DirectoryReshape,
        action: paranoic_core::MappedRecoveryAction) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { directory_reshape::recover(self, source, target, original, recovery, layout, action) }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { let _ = (source, target, original, recovery, layout, action);
          Err(CoreError::new(ErrorCode::Unsupported, "directory reshape recovery unavailable")) }
    }

    fn set_directory_modified(&self, path: &ResourcePath, unix_ms: u64) -> Result<()> {
        self.writable()?;
        #[cfg(unix)]
        {
            let (parent, name) = self.parent(path)?;
            require_kind(&parent, name, EntryKind::Directory)?;
            rustix::fs::utimensat(&parent, name, &times(unix_ms), rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| CoreError::io(e.into()))
        }
        #[cfg(not(unix))]
        { let _ = (path, unix_ms); Err(CoreError::new(ErrorCode::Unsupported, "timestamps unavailable")) }
    }

    fn prepare_move_source(&self, path: &ResourcePath) -> Result<Box<dyn paranoic_core::MoveSource>> {
        self.writable()?;
        #[cfg(unix)]
        {
            let (parent, name) = self.parent(path)?;
            let file = open_regular(&parent, name)?;
            let identity = Identity::of_fd(&file)?;
            Ok(Box::new(MoveLease { parent, name: name.to_owned(), reader: LocalReader(file), identity }))
        }
        #[cfg(not(unix))]
        { let _ = path; Err(CoreError::new(ErrorCode::Unsupported, "storage cannot pin a source version")) }
    }

    fn pin_destination(&self, path: &ResourcePath) -> Result<Box<dyn paranoic_core::PinnedDestination>> {
        #[cfg(unix)]
        {
            let (parent, name) = self.parent(path)?;
            let file = open_regular(&parent, name)?;
            let identity = Identity::of_fd(&file)?;
            Ok(Box::new(PinnedFile { parent, name: name.to_owned(), reader: LocalReader(file), identity }))
        }
        #[cfg(not(unix))]
        { let _ = path; Err(CoreError::new(ErrorCode::Unsupported, "storage cannot protect a published destination")) }
    }

    fn delete_file(&self, path: &ResourcePath) -> Result<()> {
        self.writable()?;
        let (parent, name) = self.parent(path)?;
        require_kind(&parent, name, EntryKind::File)?;
        parent.remove_file(name).map_err(CoreError::io)?;
        sync_directory(&parent)
    }

    fn delete_empty_directory(&self, path: &ResourcePath) -> Result<()> {
        self.writable()?;
        let (parent, name) = self.parent(path)?;
        require_kind(&parent, name, EntryKind::Directory)?;
        // Never recursively traverse a directory during deletion.
        parent.remove_dir(name).map_err(|error| if error.kind() == std::io::ErrorKind::DirectoryNotEmpty {
            CoreError::new(ErrorCode::InvalidInput, "folder is not empty")
        } else { CoreError::io(error) })?;
        sync_directory(&parent)
    }
}

fn require_kind(directory: &Dir, name: &str, kind: EntryKind) -> Result<()> {
    if convert_metadata(directory.symlink_metadata(name).map_err(CoreError::io)?).kind == kind {
        Ok(())
    } else {
        Err(CoreError::new(ErrorCode::Unsupported, "entry type is not supported by this operation"))
    }
}

fn convert_metadata(metadata: cap_std::fs::Metadata) -> Metadata {
    let kind = if metadata.file_type().is_symlink() { EntryKind::Symlink }
        else if metadata.is_file() { EntryKind::File }
        else if metadata.is_dir() { EntryKind::Directory }
        else { EntryKind::Other };
    let modified_unix_ms = metadata.modified().ok()
        .and_then(|time| time.into_std().duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    Metadata { kind, bytes: metadata.len(), modified_unix_ms }
}

fn sync_directory(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        // cap-std can hold an O_PATH descriptor for the granted root. Duplicating
        // that descriptor preserves O_PATH, on which fsync fails with EBADF.
        // Reopen the held directory itself, without resolving an ambient path.
        let fd = rustix::fs::openat(directory, ".",
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty()).map_err(|e| CoreError::io(e.into()))?;
        std::fs::File::from(fd).sync_all().map_err(CoreError::io)
    }
    #[cfg(not(unix))]
    {
        // No portable Windows directory fsync. This limitation is surfaced as
        // durable_directory_sync=false, never advertised as power-loss safety.
        let _ = directory;
        Ok(())
    }
}

struct LocalReader(File);

impl Read for LocalReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> { self.0.read(buffer) }
}

impl ReadHandle for LocalReader {
    fn metadata(&self) -> Result<Metadata> {
        self.0.metadata().map(convert_metadata).map_err(CoreError::io)
    }
    fn seek_to(&mut self, offset: u64) -> Result<()> {
        use std::io::Seek;
        self.0.seek(std::io::SeekFrom::Start(offset)).map(|_| ()).map_err(CoreError::io)
    }
}

struct LocalStage {
    parent: Dir,
    staging: Dir,
    temporary: String,
    target: String,
    file: Option<File>,
    cleaned: bool,
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    shared: bool,
}

impl LocalStage {
    fn cleanup(&mut self) -> Result<()> {
        self.file.take();
        self.staging.remove_file("payload").map_err(CoreError::io)?;
        self.parent.remove_dir(&self.temporary).map_err(CoreError::io)?;
        self.cleaned = true;
        sync_directory(&self.parent)
    }
    /// The payload was renamed into place: only the empty staging folder remains.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn cleanup_after_move(&mut self) -> Result<()> {
        self.parent.remove_dir(&self.temporary).map_err(CoreError::io)?;
        self.cleaned = true;
        sync_directory(&self.parent)
    }
}

impl Write for LocalStage {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().ok_or_else(closed_writer)?.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().ok_or_else(closed_writer)?.flush()
    }
}

impl StagedWrite for LocalStage {
    fn publish(mut self: Box<Self>) -> Result<()> {
        let mut file = self.file.take().ok_or_else(|| CoreError::new(ErrorCode::Internal, "staged writer is closed"))?;
        file.flush().map_err(CoreError::io)?;
        file.sync_all().map_err(CoreError::io)?;
        drop(file);
        // Atomic create-if-absent: neither an ordinary file nor a dangling
        // symlink can be replaced; a racy exists()+rename() is never used.
        // Linux/Android: renameat2(RENAME_NOREPLACE) — Android forbids hard
        // links in app data (SELinux), so link() alone made every write there
        // fail. Filesystems without RENAME_NOREPLACE fall back to link().
        #[cfg(any(target_os = "linux", target_os = "android"))]
        match crate::shared::rename_noreplace(&self.staging, "payload", &self.parent, self.target.as_str(), self.shared) {
            Ok(()) => {
                sync_directory(&self.parent)?;
                return self.cleanup_after_move();
            }
            Err(e) if e == rustix::io::Errno::INVAL || e == rustix::io::Errno::NOSYS => {}
            Err(e) => {
                let error = std::io::Error::from(e);
                if error.kind() == std::io::ErrorKind::AlreadyExists { self.cleanup()?; }
                return Err(CoreError::io(error));
            }
        }
        if let Err(error) = self.staging.hard_link("payload", &self.parent, &self.target) {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                self.cleanup()?;
            }
            return Err(CoreError::io(error));
        }
        sync_directory(&self.parent)?;
        self.cleanup()
    }

    fn abort(mut self: Box<Self>) -> Result<()> {
        self.cleanup()
    }

    fn set_modified(&mut self, unix_ms: u64) -> Result<()> {
        #[cfg(unix)]
        {
            let file = self.file.as_ref().ok_or_else(|| CoreError::new(ErrorCode::Internal, "staged writer is closed"))?;
            rustix::fs::futimens(file, &times(unix_ms)).map_err(|e| CoreError::io(e.into()))
        }
        #[cfg(not(unix))]
        { let _ = unix_ms; Err(CoreError::new(ErrorCode::Unsupported, "timestamps unavailable")) }
    }
}

impl Drop for LocalStage {
    fn drop(&mut self) {
        // Close before unlinking for Windows. Cleanup only ever touches the
        // reserved payload inside this held directory, never the destination.
        self.file.take();
        if self.cleaned { return; }
        let _ = self.staging.remove_file("payload");
        let _ = self.parent.remove_dir(&self.temporary);
    }
}

fn closed_writer() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "staged writer is closed")
}

struct LocalBundle {
    parent: Dir, directory: Dir, temporary: String, target: String,
    content: String, sidecar: String, file: Option<File>, finished: bool,
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    shared: bool,
}
impl LocalBundle {
    fn cleanup(&mut self) -> Result<()> {
        self.file.take();
        for name in [&self.content, &self.sidecar] {
            match self.directory.remove_file(name) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(CoreError::io(e)),
            }
        }
        self.parent.remove_dir(&self.temporary).map_err(CoreError::io)?;
        self.finished = true;
        sync_directory(&self.parent)
    }
}
impl Write for LocalBundle {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().ok_or_else(closed_writer)?.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().ok_or_else(closed_writer)?.flush()
    }
}
impl StagedWrite for LocalBundle {
    fn publish(mut self: Box<Self>) -> Result<()> {
        let mut file = self.file.take().ok_or_else(|| CoreError::new(ErrorCode::Internal, "bundle is closed"))?;
        file.flush().map_err(CoreError::io)?;
        file.sync_all().map_err(CoreError::io)?;
        drop(file);
        sync_directory(&self.directory)?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            crate::shared::rename_noreplace(&self.parent, self.temporary.as_str(), &self.parent,
                self.target.as_str(), self.shared)
                .map_err(|e| CoreError::io(std::io::Error::from(e)))?;
            // Never clean up through the held directory after it became public.
            self.finished = true;
            sync_directory(&self.parent)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        { Err(CoreError::new(ErrorCode::Unsupported, "atomic bundles unavailable")) }
    }
    fn abort(mut self: Box<Self>) -> Result<()> { self.cleanup() }
}
impl Drop for LocalBundle {
    fn drop(&mut self) { if !self.finished { let _ = self.cleanup(); } }
}

#[cfg(unix)]
fn times(unix_ms: u64) -> rustix::fs::Timestamps {
    let seconds = i64::try_from(unix_ms / 1000).unwrap_or(i64::MAX);
    let nanos = i64::try_from((unix_ms % 1000) * 1_000_000).unwrap_or(0);
    rustix::fs::Timestamps {
        last_access: rustix::fs::Timespec { tv_sec: 0, tv_nsec: rustix::fs::UTIME_OMIT },
        last_modification: rustix::fs::Timespec { tv_sec: seconds, tv_nsec: nanos as _ },
    }
}

#[cfg(unix)]
fn open_regular(parent: &Dir, name: &str) -> Result<File> {
    use cap_std::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
    let file = parent.open_with(name, &options).map_err(CoreError::io)?;
    if !file.metadata().map_err(CoreError::io)?.is_file() {
        return Err(CoreError::new(ErrorCode::Unsupported, "entry is not a regular file"));
    }
    Ok(file)
}

/// Object identity plus change markers. ctime cannot be set by unprivileged
/// writers, so any content or metadata change after the lease is detected.
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity { dev: u64, ino: u64, size: i64, mtime: (i64, i64), ctime: (i64, i64) }

#[cfg(unix)]
impl Identity {
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        #[allow(clippy::unnecessary_cast)]
        Self { dev: stat.st_dev as u64, ino: stat.st_ino as u64, size: stat.st_size as i64,
            mtime: (stat.st_mtime as i64, stat.st_mtime_nsec as i64), ctime: (stat.st_ctime as i64, stat.st_ctime_nsec as i64) }
    }
    fn of_fd(file: &File) -> Result<Self> {
        rustix::fs::fstat(file).map(|stat| Self::from_stat(&stat)).map_err(|e| CoreError::io(e.into()))
    }
    fn at(parent: &Dir, name: &str) -> Result<Self> {
        rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map(|stat| Self::from_stat(&stat)).map_err(|e| CoreError::io(e.into()))
    }
}

/// Source lease for cross-storage moves. remove() unlinks the name only if it
/// still refers to the leased, unmodified object. Residual race: a replacement
/// between this final check and unlink() (microseconds) is not excluded, and
/// writers holding an older open handle can still write to the unlinked inode.
#[cfg(unix)]
struct MoveLease { parent: Dir, name: String, reader: LocalReader, identity: Identity }

#[cfg(unix)]
impl paranoic_core::MoveSource for MoveLease {
    fn reader(&mut self) -> &mut dyn ReadHandle { &mut self.reader }
    fn remove(self: Box<Self>) -> Result<()> {
        if Identity::of_fd(&self.reader.0)? != self.identity || Identity::at(&self.parent, &self.name)? != self.identity {
            return Err(CoreError::new(ErrorCode::SourceChanged, "source changed during move; it was not removed"));
        }
        self.parent.remove_file(&self.name).map_err(CoreError::io)?;
        sync_directory(&self.parent)
    }
}

#[cfg(unix)]
struct PinnedFile { parent: Dir, name: String, reader: LocalReader, identity: Identity }

#[cfg(unix)]
impl paranoic_core::PinnedDestination for PinnedFile {
    fn reader(&mut self) -> &mut dyn ReadHandle { &mut self.reader }
    fn still_published(&mut self) -> Result<()> {
        if Identity::of_fd(&self.reader.0)? != self.identity || Identity::at(&self.parent, &self.name)? != self.identity {
            return Err(CoreError::new(ErrorCode::Integrity, "destination changed after verification; source kept"));
        }
        Ok(())
    }
}

