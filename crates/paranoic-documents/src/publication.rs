use std::{io::{Read, Write, Seek, SeekFrom}, rc::Rc, cell::RefCell};
use paranoic_core::{Core, Location, JobId, JobRecord, Operation, Storage, ResourcePath, StorageDomain, EntryKind};
use paranoic_crypto::{StreamSink, CryptoError};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Result, TextDocument, ZipService, ZipSource, ArchiveLimits};

/// New private seekable output; commit publishes only its spool artifact.
/// Drop and abort must discard unpublished output. Commit failure is uncertain.
pub trait ArchiveOutput: Read + Write + Seek {
    fn commit(self: Box<Self>) -> Result<()>;
    fn abort(self: Box<Self>) -> Result<()>;
}

/// Digest/length of a fully prepared output. No password or plaintext retained.
#[derive(Clone, Debug)]
pub struct PreparedArtifact { bytes: u64, digest: [u8;32] }
impl PreparedArtifact {
    pub fn bytes(&self) -> u64 { self.bytes }
    pub fn blake3(&self) -> &[u8;32] { &self.digest }
    /// A create-new, journaled publication which retains its spool source.
    /// Persist job_id first and retry exactly the same request after lost replies.
    /// "Save" over an existing file: the job replaces `target` only if it still
    /// has exactly `expected_target` (e.g. `TextDocument::opened_metadata`) and
    /// only with this artifact's bytes. A file edited elsewhere in the meantime
    /// fails with SourceChanged instead of being overwritten.
    pub fn submit_replace(&self, core: &Core, spool: Location, target: Location, expected_target: paranoic_core::Metadata,
        job_id: JobId) -> Result<JobRecord> {
        core.submit_with_id(job_id, Operation::ReplaceExpected { source: spool, target, bytes: self.bytes,
            blake3: self.digest, expected_target })
    }
    pub fn submit_new(&self, core: &Core, spool: Location, target: Location, job_id: JobId) -> Result<JobRecord> {
        core.submit_with_id(job_id, Operation::CopyExpected { source: spool, target,
            bytes: self.bytes, blake3: self.digest })
    }
    /// Checked editor save: verify the old contents after isolating the old
    /// file as a backup, before publishing the prepared replacement.
    pub fn submit_replace_content(&self, core: &Core, spool: Location, target: Location,
        expected_target: paranoic_core::Metadata, expected_target_blake3: [u8; 32], job_id: JobId) -> Result<JobRecord> {
        core.submit_with_id(job_id, Operation::ReplaceContentExpected { source: spool, target, bytes: self.bytes,
            blake3: self.digest, expected_target, expected_target_blake3 })
    }
}
impl ZipService<std::io::Cursor<Vec<u8>>> {
    /// Own preparation, cleanup and spool commit as one operation. Never publish
    /// the final user destination here: use PreparedArtifact::submit_new.
    pub fn create_prepared(mut output: Box<dyn ArchiveOutput>, sources: Vec<ZipSource>, limits: ArchiveLimits,
        password: Option<Zeroizing<String>>, mut progress: impl FnMut(u64) -> Result<()>) -> Result<PreparedArtifact> {
        let result = (|| {
            Self::create(&mut output, sources, limits, password, &mut progress)?;
            let bytes = output.seek(SeekFrom::End(0)).map_err(CoreError::io)?;
            if bytes > limits.archive_bytes { return Err(CoreError::new(ErrorCode::LimitExceeded, "prepared ZIP exceeds limit")); }
            output.seek(SeekFrom::Start(0)).map_err(CoreError::io)?;
            let mut hash = blake3::Hasher::new(); let mut count = 0u64;
            let mut buffer = Zeroizing::new(vec![0u8;65536]);
            loop {
                progress(count)?;
                let n = output.read(&mut buffer).map_err(CoreError::io)?;
                if n == 0 { break; }
                count = count.checked_add(n as u64).ok_or_else(|| CoreError::new(ErrorCode::LimitExceeded, "ZIP size overflow"))?;
                if count > bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "prepared ZIP changed")); }
                hash.update(&buffer[..n]);
            }
            if count != bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "prepared ZIP changed")); }
            progress(count)?;
            Ok(PreparedArtifact { bytes, digest: *hash.finalize().as_bytes() })
        })();
        match result {
            Ok(artifact) => { output.commit().map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "ZIP spool publication uncertain"))?; Ok(artifact) }
            Err(error) => { output.abort()?; Err(error) }
        }
    }
}
impl TextDocument {
    /// Read from an explicit Core grant, checking opened-object metadata/EOF.
    pub fn open_location(core: &Core, source: &Location, limit: usize,
        mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        check()?;
        let mut input = core.open_read(source)?;
        let before = input.metadata()?;
        if before.kind != EntryKind::File || before.bytes > limit as u64 {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "document size or kind unsupported"));
        }
        let mut counted = CountedRead { input: &mut input, bytes: 0 };
        let document = Self::open(&mut counted, limit, &mut check)?;
        if counted.bytes != before.bytes {
            return Err(CoreError::new(ErrorCode::SourceChanged, "document length changed while opening"));
        }
        if input.metadata()? != before || core.metadata(source)? != before {
            return Err(CoreError::new(ErrorCode::SourceChanged, "document changed while opening"));
        }
        check()?;
        let mut document = document;
        document.opened = Some(before);
        Ok(document)
    }
    /// Save As to a new path through the storage adapter's atomic staged writer.
    pub fn save_as(&self, storage: &dyn Storage, target: &ResourcePath,
        check: impl FnMut() -> Result<()>) -> Result<()> {
        if storage.domain() != StorageDomain::Files || target.is_root() || !storage.capabilities().write {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "document destination unavailable"));
        }
        self.save(Box::new(StorageSink(storage.stage(target, JobId(uuid::Uuid::new_v4()))?)), check)
    }
    /// Serialize a revision into a private transactional spool and retain exact
    /// evidence for later publication via Core without persisting plaintext in DB.
    pub fn prepare_save(&self, output: Box<dyn StreamSink>, check: impl FnMut() -> Result<()>) -> Result<PreparedArtifact> {
        let receipt = Rc::new(RefCell::new(None));
        self.save(Box::new(RecordingSink { output, hash: blake3::Hasher::new(), bytes: 0, receipt: receipt.clone() }), check)?;
        let artifact = receipt.borrow_mut().take().ok_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "document spool outcome unknown"))?;
        Ok(artifact)
    }
}
struct StorageSink(Box<dyn paranoic_core::StagedWrite>);
impl Write for StorageSink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.write(b) }
    fn flush(&mut self) -> std::io::Result<()> { self.0.flush() }
}
impl StreamSink for StorageSink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.publish().map_err(|_| CryptoError::PublicationUncertain) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.abort().map_err(|_| CryptoError::CleanupFailed) }
}
struct RecordingSink {
    output: Box<dyn StreamSink>, hash: blake3::Hasher, bytes: u64,
    receipt: Rc<RefCell<Option<PreparedArtifact>>>,
}
impl Write for RecordingSink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.output.write(b)?;
        if n > b.len() { return Err(std::io::Error::other("invalid write count")); }
        self.bytes = self.bytes.checked_add(n as u64).ok_or_else(|| std::io::Error::other("document size overflow"))?;
        self.hash.update(&b[..n]); Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.output.flush() }
}
impl StreamSink for RecordingSink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> {
        let Self { output, hash, bytes, receipt } = *self;
        output.commit()?;
        *receipt.borrow_mut() = Some(PreparedArtifact { bytes, digest: *hash.finalize().as_bytes() }); Ok(())
    }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.output.abort() }
}

struct CountedRead<R> { input: R, bytes: u64 }
impl<R: Read> Read for CountedRead<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.input.read(buffer)?;
        if count > buffer.len() { return Err(std::io::Error::other("invalid read count")); }
        self.bytes = self.bytes.checked_add(count as u64).ok_or_else(|| std::io::Error::other("read size overflow"))?;
        Ok(count)
    }
}
