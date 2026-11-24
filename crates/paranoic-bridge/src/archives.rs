//! Plan 2.1: archives — ZIP/ZIP64 (with AES) and 7z. The format is chosen
//! explicitly by the caller (no guessing). Listing and extraction read the
//! archive through Core; 7z decoding runs only in the confined worker process
//! (`IsolatedSevenZip`, register У-5). Extraction is prepared in a private
//! spool folder, moved into place by one journaled Core job that checks the
//! exact file set, lengths and BLAKE3, then read back and verified. Creation
//! writes to a private spool file and copies it out as a new file.
use crate::*;
use features::{finish, run_wait};
use paranoic_core::{EntryKind, ResourcePath};
use paranoic_documents::{ArchiveLimits, CodecLimits, IsolatedSevenZip, SevenZipService, ZipService, ZipSource};
use std::io::{Read, Seek, SeekFrom, Write};
use zeroize::Zeroizing;

/// `kind` of the archive calls.
pub const ARCHIVE_ZIP: u8 = 1;
pub const ARCHIVE_7Z: u8 = 2;
const CREATE_ENTRY_LIMIT: usize = 10_000;

#[derive(Clone, uniffi::Record)]
/// `encrypted`: the entry needs the archive password (E2.1).
pub struct NativeArchiveEntry { pub path: String, pub folder: bool, pub size: u64, pub compressed: u64, pub encrypted: bool }

/// Random access over a Core reader for archive libraries.
struct CoreSeek { reader: archive_io::Reader, position: u64, length: u64, control: archive_work::ArchiveControl }
impl CoreSeek {
    fn open(core: &paranoic_core::Core, location: &Location, control: archive_work::ArchiveControl) -> Result<Self> {
        control.check()?;
        let reader = archive_io::Reader::open(readable(core, location)?, location.path.clone(), None, control.clone())?;
        let meta = reader.metadata.clone();
        if meta.kind != EntryKind::File { return Err(BridgeError::Unsupported); }
        Ok(Self { reader, position: 0, length: meta.bytes, control })
    }
}
impl Read for CoreSeek {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.control.check().map_err(|_| std::io::Error::other("archive preparation stopped"))?;
        let n = self.reader.read(buffer)?;
        self.control.check().map_err(|_| std::io::Error::other("archive preparation stopped"))?;
        self.position += n as u64;
        Ok(n)
    }
}
impl Seek for CoreSeek {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.control.check().map_err(|_| std::io::Error::other("archive preparation stopped"))?;
        let next = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.length.checked_add_signed(d),
            SeekFrom::Current(d) => self.position.checked_add_signed(d),
        }.ok_or_else(|| std::io::Error::other("seek out of range"))?;
        if next != self.position { self.reader.seek_to(next).map_err(|_| std::io::Error::other("seek unavailable"))?; }
        self.position = next;
        Ok(next)
    }
}

fn entries(list: &[paranoic_documents::ArchiveEntry]) -> Vec<NativeArchiveEntry> {
    list.iter().map(|e| NativeArchiveEntry { path: e.path.as_str().to_owned(), folder: e.directory, size: e.bytes,
        compressed: e.compressed_bytes, encrypted: e.encrypted }).collect()
}
fn secret(password: Option<String>) -> Option<Zeroizing<String>> { password.filter(|p| !p.is_empty()).map(Zeroizing::new) }

/// Same admission checks as Core's synchronous read helpers. The blocking
/// provider call itself runs under the bounded, cancellable I/O wait.
fn readable(core: &Core, location: &Location) -> Result<Arc<dyn Storage>> {
    core.health()?;
    let storage = core.storage(location.storage)?;
    if !storage.capabilities().read { return Err(BridgeError::Denied); }
    Ok(storage)
}
fn source_metadata(core: &Core, location: &Location, control: &archive_work::ArchiveControl) -> Result<Metadata> {
    let storage = readable(core, location)?;
    let path = location.path.clone();
    Ok(archive_io::run(control, move || storage.metadata(&path))?)
}

/// Enumeration retains a source description, never an open descriptor.
/// A codec opens this file on first read and releases it at EOF/drop.
pub(crate) struct DeferredArchiveSource {
    storage: Arc<dyn Storage>, path: ResourcePath, expected: Metadata,
    input: Option<archive_io::Reader>, ended: bool, control: archive_work::ArchiveControl,
}
impl DeferredArchiveSource {
    pub(crate) fn new(storage: Arc<dyn Storage>, path: ResourcePath, expected: Metadata) -> Self {
        Self { storage, path, expected, input: None, ended: false, control: archive_work::ArchiveControl::new() }
    }
    fn with_control(mut self, control: archive_work::ArchiveControl) -> Self {
        self.control = control; self
    }
}
impl Read for DeferredArchiveSource {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() || self.ended { return Ok(0); }
        let changed = || std::io::Error::other("archive source changed before or during reading");
        if self.input.is_none() {
            let input = archive_io::Reader::open(Arc::clone(&self.storage), self.path.clone(),
                Some(self.expected.clone()), self.control.clone()).map_err(|_| changed())?;
            self.input = Some(input);
        }
        let input = self.input.as_mut().ok_or_else(changed)?;
        let count = input.read(buffer)?;
        if count == 0 {
            // EOF/descriptor/path metadata were checked by the I/O worker.
            self.input = None;
            self.ended = true;
        }
        Ok(count)
    }
}

/// Removes the private copy of a non-seekable archive source when the
/// operation ends. Declared before the source, so it drops after it.
struct SourceCopy<'a> { runtime: &'a NativeRuntime, path: Option<ResourcePath> }
impl Drop for SourceCopy<'_> {
    fn drop(&mut self) { if let Some(path) = self.path.take() { self.runtime.drop_spool(&path); } }
}

impl NativeRuntime {
    /// The archive itself when it allows random access; otherwise (pipe-backed
    /// provider streams, У-61) a sequential copy in the private workspace,
    /// checked for the exact length and an unchanged source, removed by `copy`.
    fn archive_source<'a>(&'a self, location: &Location, control: &archive_work::ArchiveControl,
        copy: &mut SourceCopy<'a>) -> Result<CoreSeek> {
        let mut direct = CoreSeek::open(&self.core, location, control.clone())?;
        match direct.reader.probe_random_access() {
            Ok(()) => return Ok(direct),
            Err(error) if error.code == paranoic_core::ErrorCode::Unsupported => {}
            Err(error) => return Err(error.into()),
        }
        drop(direct); // releases its I/O slot before the copy takes one
        let mut input = archive_io::Reader::open(readable(&self.core, location)?, location.path.clone(), None, control.clone())?;
        let expected = input.metadata.bytes;
        if input.metadata.kind != EntryKind::File { return Err(BridgeError::Unsupported); }
        if expected > ArchiveLimits::default().archive_bytes { return Err(BridgeError::Limit); }
        let spool = Self::spool_path()?;
        copy.path = Some(spool.clone());
        let mut staged = self.app.stage(&spool, paranoic_core::JobId(Uuid::new_v4()))?;
        let copied = (|| -> Result<()> {
            let mut total = 0u64;
            loop {
                control.check()?;
                let bytes = input.read_chunk(1 << 20)?;
                if bytes.is_empty() { break; } // the I/O worker re-checked the source at EOF
                total += bytes.len() as u64;
                if total > expected { return Err(BridgeError::Changed); }
                staged.write_all(&bytes).map_err(paranoic_core::CoreError::io)?;
            }
            if total != expected { return Err(BridgeError::Changed); }
            Ok(())
        })();
        drop(input);
        match copied {
            Ok(()) => staged.publish()?,
            Err(error) => { let _ = staged.abort(); return Err(error); }
        }
        CoreSeek::open(&self.core, &Location { storage: self.app.id(), path: spool }, control.clone())
    }
    /// Files (and folders, recursively) selected by the user as archive sources.
    fn collect_sources(&self, location: &Location, prefix: &ResourcePath, out: &mut Vec<ZipSource>, depth: usize,
        control: &archive_work::ArchiveControl) -> Result<()> {
        control.check()?;
        if out.len() >= CREATE_ENTRY_LIMIT || depth > 64 { return Err(BridgeError::Limit); }
        let meta = source_metadata(&self.core, location, control)?;
        match meta.kind {
            EntryKind::File => {
                let reader = DeferredArchiveSource::new(readable(&self.core, location)?, location.path.clone(), meta.clone())
                    .with_control(control.clone());
                out.push(ZipSource { path: prefix.clone(), bytes: meta.bytes, reader: Some(Box::new(reader)) });
            }
            EntryKind::Directory => {
                out.push(ZipSource { path: prefix.clone(), bytes: 0, reader: None });
                let storage = readable(&self.core, location)?;
                let path = location.path.clone();
                // NativeRuntime is opened with default CoreConfig (10,000),
                // matching the archive entry budget and Core::list admission.
                let children = archive_io::run(control, move || storage.list(&path, CREATE_ENTRY_LIMIT))?;
                for child in children {
                    let name = child.location.path.parent_and_name()?.1.to_owned();
                    self.collect_sources(&child.location, &prefix.join(&name)?, out, depth + 1, control)?;
                }
            }
            _ => return Err(BridgeError::Unsupported),
        }
        Ok(())
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Legacy entry points retain their ABI; current hosts use request-bound calls.
    pub fn archive_list(&self, archive: NativeLocation, kind: u8, password: Option<String>) -> Result<Vec<NativeArchiveEntry>> {
        self.archive_list_with_request(archive, kind, password, Uuid::new_v4().to_string())
    }
    pub fn archive_extract(&self, archive: NativeLocation, kind: u8, password: Option<String>, target_parent: NativeLocation,
        name: String) -> Result<()> {
        self.archive_extract_with_request(archive, kind, password, target_parent, name, Uuid::new_v4().to_string())
    }
    pub fn archive_create(&self, sources: Vec<NativeLocation>, kind: u8, password: Option<String>, target_parent: NativeLocation,
        name: String) -> Result<()> {
        self.archive_create_with_request(sources, kind, password, target_parent, name, Uuid::new_v4().to_string())
    }
    pub fn archive_list_with_request(&self, archive: NativeLocation, kind: u8, password: Option<String>,
        request_id: String) -> Result<Vec<NativeArchiveEntry>> {
        let preparation = self.archive_work.begin(&request_id)?;
        let control = &preparation.control;
        let mut copy = SourceCopy { runtime: self, path: None };
        let source = self.archive_source(&archive.core()?, control, &mut copy)?;
        let result = (|| -> Result<_> {
            match kind {
                ARCHIVE_ZIP => Ok(entries(ZipService::open(source, ArchiveLimits::default(), || control.check())?.entries())),
                ARCHIVE_7Z => Ok(entries(IsolatedSevenZip::open_with_host(source, CodecLimits::default(), secret(password),
                    &self.pdf_worker, Arc::clone(&control.cancelled), self.document_worker_host()?, || control.check())?.entries())),
                _ => Err(BridgeError::Invalid),
            }
        })();
        control.check()?; result
    }
    /// Decode completely into private storage, then publish with the same ID.
    pub fn archive_extract_with_request(&self, archive: NativeLocation, kind: u8, password: Option<String>,
        target_parent: NativeLocation, name: String, request_id: String) -> Result<()> {
        let preparation = self.archive_work.begin(&request_id)?;
        let control = &preparation.control;
        control.check()?;
        let mut copy = SourceCopy { runtime: self, path: None };
        let source = self.archive_source(&archive.core()?, control, &mut copy)?;
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let target_storage = readable(&self.core, &target)?;
        let target_path = target.path.clone();
        match archive_io::run(control, move || target_storage.metadata(&target_path)) {
            Ok(_) => return Err(BridgeError::Conflict),
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound => {},
            Err(error) => return Err(error.into()),
        }
        let mut spool = spool::PublicationSpool::new(self)?;
        let decoded = match kind {
            ARCHIVE_ZIP => ZipService::open(source, ArchiveLimits::default(), || control.check())
                .and_then(|mut zip| zip.prepare_extraction(self.app.as_ref(), spool.path(), secret(password), |_| control.check())),
            ARCHIVE_7Z => IsolatedSevenZip::open_with_host(source, CodecLimits::default(), secret(password),
                &self.pdf_worker, Arc::clone(&control.cancelled), self.document_worker_host()?, || control.check())
                .and_then(|sz| sz.prepare_extraction(self.app.as_ref(), spool.path(), |_| control.check())),
            _ => return Err(BridgeError::Invalid),
        };
        control.check()?;
        let prepared = decoded?;
        let source = spool.handoff();
        let job = if target.storage == self.app.id() {
            self.core.submit_with_id(preparation.id, Operation::Move { source, target: target.clone() })?
        } else {
            prepared.submit_new(&self.core, source, target.clone(), preparation.id)?
        };
        preparation.publication_started(&self.core);
        finish(run_wait(&self.core, job.id)?, 1)?;
        prepared.verify_published(&self.core, &target, &std::sync::atomic::AtomicBool::new(false))?;
        spool.completed();
        Ok(())
    }
    pub fn archive_create_with_request(&self, sources: Vec<NativeLocation>, kind: u8, password: Option<String>,
        target_parent: NativeLocation, name: String, request_id: String) -> Result<()> {
        let preparation = self.archive_work.begin(&request_id)?;
        let control = &preparation.control;
        control.check()?;
        if sources.is_empty() { return Err(BridgeError::Invalid); }
        if kind == ARCHIVE_7Z && password.as_ref().is_some_and(|p| !p.is_empty()) { return Err(BridgeError::Unsupported); }
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let mut collected = Vec::new();
        for source in sources {
            let source = source.core()?;
            let base = ResourcePath::parse(source.path.parent_and_name()?.1.to_owned())?;
            self.collect_sources(&source, &base, &mut collected, 0, control)?;
        }
        let mut spool = spool::PublicationSpool::new(self)?;
        let file_path = self.workspace_root.join(spool.path().as_str());
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut output = options.open(&file_path).map_err(|_| BridgeError::Unavailable)?;
        let encoded = match kind {
            ARCHIVE_ZIP => ZipService::create(&mut output, collected, ArchiveLimits::default(), secret(password), |_| control.check()),
            ARCHIVE_7Z => SevenZipService::create(&mut output, collected, CodecLimits::default(), |_| control.check()),
            _ => return Err(BridgeError::Invalid),
        };
        control.check()?;
        encoded?;
        output.sync_all().map_err(|_| BridgeError::Unavailable)?;
        drop(output);
        control.check()?;
        let job = self.core.submit_with_id(preparation.id, Operation::CopyFile { source: spool.handoff(),
            target, conflict: paranoic_core::ConflictPolicy::Fail })?;
        preparation.publication_started(&self.core);
        finish(run_wait(&self.core, job.id)?, 1)?;
        spool.completed();
        Ok(())
    }
}
