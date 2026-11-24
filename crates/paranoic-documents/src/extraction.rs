//! Archive extraction as a journaled Core job.
//!
//! Phase 1 (`prepare_*`): decode into a NEW directory on a private spool grant,
//! recording path, length and BLAKE3 of every file. Failed preparation removes
//! only the spool directory it created; uncertain cleanup is NeedsAttention.
//! Phase 2 (`submit_new`): Core `MoveDirectoryExpected` copies the spool tree to
//! the user destination (create-new, never merged), requires the copy to equal
//! the manifest exactly, re-verifies each file and removes the spool. The job is persisted and resumable/inspectable like any
//! other Core job; it is never re-run automatically after an uncertain result.
//! Phase 3 (`verify_published`): compare the published tree with the manifest.
use std::collections::BTreeMap;
use paranoic_core::{Core, EntryKind, JobId, JobRecord, Location, Operation, ResourcePath, Storage, StorageDomain};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Result, SevenZipService, ZipService};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedFile { pub path: ResourcePath, pub bytes: u64, pub blake3: [u8; 32] }

#[derive(Default)]
pub(crate) struct Recorder { pub(crate) files: Vec<ExtractedFile>, pub(crate) directories: Vec<ResourcePath> }

/// Manifest of a completely spooled extraction. Holds no password or content.
#[derive(Clone, Debug)]
pub struct PreparedExtraction { files: Vec<ExtractedFile>, directories: Vec<ResourcePath>, bytes: u64 }

impl PreparedExtraction {
    pub fn files(&self) -> &[ExtractedFile] { &self.files }
    /// Relative directories, including the root (empty path).
    pub fn directories(&self) -> &[ResourcePath] { &self.directories }
    pub fn bytes(&self) -> u64 { self.bytes }

    /// Persist `job_id` before calling; after a lost reply query `core.job(job_id)`
    /// instead of submitting again. The destination must not exist.
    pub fn submit_new(&self, core: &Core, spool: Location, target: Location, job_id: JobId) -> Result<JobRecord> {
        if spool.path.is_root() || target.path.is_root() {
            return Err(CoreError::new(ErrorCode::InvalidInput, "extraction spool and destination must be named directories"));
        }
        // The manifest travels inside the journaled job: Core compares the copied
        // tree with it before any spool file is removed.
        let files = self.files.iter().map(|f| paranoic_core::ExpectedTreeFile { path: f.path.clone(), bytes: f.bytes, blake3: f.blake3 }).collect();
        core.submit_with_id(job_id, Operation::MoveDirectoryExpected { source: spool, target, files })
    }

    /// Read back the published tree through Core and require an exact match:
    /// same directory set, same file set, lengths and BLAKE3. Never mutates.
    pub fn verify_published(&self, core: &Core, target: &Location, cancelled: &std::sync::atomic::AtomicBool) -> Result<()> {
        let check = || if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            Err(CoreError::new(ErrorCode::Cancelled, "extraction verification cancelled")) } else { Ok(()) };
        let inventory = core.scan_tree(target, paranoic_core::TreeLimits::default(), cancelled)?;
        let mut expected_files: BTreeMap<&str, &ExtractedFile> = self.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut expected_dirs: std::collections::BTreeSet<&str> = self.directories.iter().map(|d| d.as_str()).collect();
        let mismatch = || CoreError::new(ErrorCode::Integrity, "published extraction differs from prepared manifest");
        let prefix = format!("{}/", target.path.as_str());
        for entry in &inventory.entries {
            check()?;
            let relative = if entry.location.path == target.path { "" } else {
                entry.location.path.as_str().strip_prefix(&prefix).ok_or_else(mismatch)? };
            match entry.metadata.kind {
                EntryKind::Directory => { if !expected_dirs.remove(relative) { return Err(mismatch()); } }
                EntryKind::File => {
                    let file = expected_files.remove(relative).ok_or_else(mismatch)?;
                    if entry.metadata.bytes != file.bytes { return Err(mismatch()); }
                    let mut input = core.open_read(&entry.location)?;
                    let mut hash = blake3::Hasher::new(); let mut count = 0u64;
                    let mut buffer = Zeroizing::new(vec![0u8; 65536]);
                    loop {
                        check()?;
                        let n = input.read(&mut buffer).map_err(CoreError::io)?;
                        if n == 0 { break; }
                        count = count.checked_add(n as u64).ok_or_else(mismatch)?;
                        if count > file.bytes { return Err(mismatch()); }
                        hash.update(&buffer[..n]);
                    }
                    if count != file.bytes || hash.finalize().as_bytes() != &file.blake3 { return Err(mismatch()); }
                }
                _ => return Err(mismatch()),
            }
        }
        if !expected_files.is_empty() || !expected_dirs.is_empty() { return Err(mismatch()); }
        Ok(())
    }
}
use std::io::Read;

pub(crate) fn spool_ready(spool: &dyn Storage, root: &ResourcePath) -> Result<()> {
    if spool.domain() != StorageDomain::Files || root.is_root() || !spool.capabilities().write || !spool.capabilities().delete {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "extraction spool unavailable"));
    }
    match spool.metadata(root) {
        Err(error) if error.code == ErrorCode::NotFound => Ok(()),
        Ok(_) => Err(CoreError::new(ErrorCode::AlreadyExists, "extraction spool already exists")),
        Err(error) => Err(error),
    }
}

/// Remove exactly what this extraction created, nothing else: recorded files
/// (only if still the recorded length) and recorded directories (only when
/// empty, deepest first; the root only if this operation created it).
/// Anything unexpected stops cleanup with NeedsAttention — never recursive.
fn discard(spool: &dyn Storage, root: &ResourcePath, record: &Recorder) -> Result<()> {
    let uncertain = || CoreError::new(ErrorCode::NeedsAttention, "extraction spool cleanup incomplete; inspect spool");
    let under = |relative: &ResourcePath| -> Result<ResourcePath> {
        if relative.is_root() { Ok(root.clone()) } else { ResourcePath::parse(format!("{}/{}", root.as_str(), relative.as_str())) }
    };
    for file in &record.files {
        let path = under(&file.path)?;
        match spool.metadata(&path) {
            Ok(m) if m.kind == EntryKind::File && m.bytes == file.bytes => spool.delete_file(&path).map_err(|_| uncertain())?,
            Err(e) if e.code == ErrorCode::NotFound => {}
            _ => return Err(uncertain()),
        }
    }
    let mut directories: Vec<&ResourcePath> = record.directories.iter().collect();
    directories.sort_by_key(|d| std::cmp::Reverse(if d.is_root() { 0 } else { d.as_str().split('/').count() }));
    for directory in directories {
        match spool.delete_empty_directory(&under(directory)?) {
            Ok(()) => {}
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(_) => return Err(uncertain()),
        }
    }
    Ok(())
}

pub(crate) fn finish(spool: &dyn Storage, root: &ResourcePath, record: Recorder, result: Result<u64>) -> Result<PreparedExtraction> {
    match result {
        Ok(bytes) => Ok(PreparedExtraction { files: record.files, directories: record.directories, bytes }),
        Err(error) => {
            discard(spool, root, &record)?;
            Err(if error.code == ErrorCode::NeedsAttention { CoreError::new(ErrorCode::InvalidInput, "archive extraction failed; spool discarded") } else { error })
        }
    }
}

impl<R: std::io::Read + std::io::Seek> ZipService<R> {
    /// Phase 1 for ZIP. `spool_root` must not exist on the private spool grant.
    pub fn prepare_extraction(&mut self, spool: &dyn Storage, spool_root: &ResourcePath,
        password: Option<Zeroizing<String>>, check: impl FnMut(u64) -> Result<()>) -> Result<PreparedExtraction> {
        spool_ready(spool, spool_root)?;
        let mut record = Recorder::default();
        let result = self.extract_recorded(spool, spool_root, password, check, &mut record);
        finish(spool, spool_root, record, result)
    }
}
impl<R: std::io::Read + std::io::Seek> SevenZipService<R> {
    /// Phase 1 for 7z. `spool_root` must not exist on the private spool grant.
    pub fn prepare_extraction(&mut self, spool: &dyn Storage, spool_root: &ResourcePath,
        check: impl FnMut(u64) -> Result<()>) -> Result<PreparedExtraction> {
        spool_ready(spool, spool_root)?;
        let mut record = Recorder::default();
        let result = self.extract_private_recorded(spool, spool_root, check, &mut record);
        finish(spool, spool_root, record, result)
    }
}
