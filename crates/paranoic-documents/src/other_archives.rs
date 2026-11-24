//! 7z codecs. RAR support was removed by owner decision (2026-11-12).
use std::{io::{Read, Write, Seek, SeekFrom}, collections::{BTreeMap, BTreeSet}};
use paranoic_core::{Storage, StorageDomain, ResourcePath, JobId, StagedWrite};
use zeroize::Zeroizing;
use sevenz_rust2::{SevenZReader, SevenZWriter, SevenZArchiveEntry, Password};
use crate::{ArchiveEntry, ArchiveLimits, ZipSource, CoreError, ErrorCode, Result};

/// Decoder-specific ceilings, independent of the archive's declared sizes.
#[derive(Clone, Copy, Debug)]
pub struct CodecLimits { pub archive: ArchiveLimits, pub pending_files: usize }
impl Default for CodecLimits {
    fn default() -> Self { Self { archive: ArchiveLimits::default(), pending_files: 128 } }
}
impl CodecLimits {
    fn validate(self) -> Result<()> {
        self.archive.validate()?;
        if self.pending_files == 0 || self.pending_files > 128 { return Err(invalid()); }
        Ok(())
    }
}

pub(crate) fn index(entries: impl IntoIterator<Item=ArchiveEntry>, limits: CodecLimits) -> Result<Vec<ArchiveEntry>> {
    limits.validate()?;
    let mut names = BTreeMap::new(); let mut result = Vec::new(); let mut total = 0u64;
    for entry in entries {
        if result.len() >= limits.archive.entries { return Err(large()); }
        crate::archive::validate_name(&mut names, &entry.path, entry.directory, limits.archive.depth)?;
        if entry.bytes > limits.archive.entry_bytes || (entry.directory && entry.bytes != 0) { return Err(large()); }
        total = total.checked_add(entry.bytes).ok_or_else(large)?;
        if total > limits.archive.total_bytes { return Err(large()); }
        result.push(entry);
    }
    Ok(result)
}

/// `record` is the caller's journal of objects this extraction created (root
/// only once created, files only once published), kept on every error path so
/// cleanup can remove exactly those and nothing else.
pub(crate) struct Destination<'a> {
    storage: &'a dyn Storage, root: ResourcePath, directories: BTreeSet<String>,
    pending: Vec<(Box<dyn StagedWrite>, crate::ExtractedFile)>, limit: usize, total: u64,
    private_spool: bool,
    record: &'a mut crate::extraction::Recorder,
}
impl<'a> Destination<'a> {
    pub(crate) fn new(storage: &'a dyn Storage, root: &ResourcePath, entries: &[ArchiveEntry], limit: usize,
        record: &'a mut crate::extraction::Recorder) -> Result<Self> {
        Self::with_mode(storage, root, entries, limit, record, false)
    }
    /// Only for an invisible private preparation directory. Batches may be
    /// committed here before the archive's final CRC, never at the user target.
    pub(crate) fn private(storage: &'a dyn Storage, root: &ResourcePath, entries: &[ArchiveEntry], limit: usize,
        record: &'a mut crate::extraction::Recorder) -> Result<Self> {
        Self::with_mode(storage, root, entries, limit, record, true)
    }
    fn with_mode(storage: &'a dyn Storage, root: &ResourcePath, entries: &[ArchiveEntry], limit: usize,
        record: &'a mut crate::extraction::Recorder, private_spool: bool) -> Result<Self> {
        if storage.domain() != StorageDomain::Files || !storage.capabilities().write || root.is_root() { return Err(invalid()); }
        if !(1..=128).contains(&limit) { return Err(invalid()); }
        if !private_spool && entries.iter().filter(|e| !e.directory).count() > limit { return Err(large()); }
        for entry in entries { ResourcePath::parse(format!("{}/{}", root.as_str(), entry.path.as_str()))?; }
        storage.create_directory(root)?;
        let mut directories = BTreeSet::new(); directories.insert(root.as_str().to_owned());
        record.directories.push(ResourcePath::root());
        Ok(Self { storage, root: root.clone(), directories, pending: Vec::new(), limit, total: 0, record, private_spool })
    }
    pub(crate) fn push(&mut self, entry: &ArchiveEntry, input: &mut dyn Read, check: &mut impl FnMut(u64)->Result<()>) -> Result<()> {
        check(self.total)?;
        let path = ResourcePath::parse(format!("{}/{}", self.root.as_str(), entry.path.as_str()))?;
        let parent = if entry.directory { path.clone() } else { path.parent_and_name()?.0 };
        let suffix = parent.as_str().strip_prefix(self.root.as_str()).ok_or_else(invalid)?;
        let mut current = self.root.clone();
        for part in suffix.trim_start_matches('/').split('/').filter(|p| !p.is_empty()) {
            current = current.join(part)?;
            if self.directories.insert(current.as_str().to_owned()) {
                self.storage.create_directory(&current)?;
                self.record.directories.push(ResourcePath::parse(current.as_str()[self.root.as_str().len()+1..].to_owned())?);
            }
        }
        if entry.directory { return Ok(()); }
        if self.pending.len() >= self.limit {
            if !self.private_spool { return Err(large()); }
            self.flush_pending(check)?;
        }
        let mut output = self.storage.stage(&path, JobId(uuid::Uuid::new_v4()))?;
        let mut hash = blake3::Hasher::new();
        let result = (|| {
            let mut bytes = 0u64; let mut buffer = Zeroizing::new(vec![0u8;65536]);
            loop {
                check(self.total)?;
                let n = input.read(&mut buffer).map_err(CoreError::io)?;
                if n == 0 { break; }
                bytes = bytes.checked_add(n as u64).ok_or_else(large)?;
                if bytes > entry.bytes { return Err(large()); }
                self.total = self.total.checked_add(n as u64).ok_or_else(large)?;
                output.write_all(&buffer[..n]).map_err(CoreError::io)?;
                hash.update(&buffer[..n]);
            }
            if bytes != entry.bytes { return Err(invalid()); }
            output.flush().map_err(CoreError::io)?; check(self.total)
        })();
        if let Err(error) = result { output.abort()?; return Err(error); }
        self.pending.push((output, crate::ExtractedFile { path: entry.path.clone(), bytes: entry.bytes, blake3: *hash.finalize().as_bytes() }));
        Ok(())
    }
    pub(crate) fn abort(mut self) -> Result<()> {
        let mut failed = false;
        for (output, _) in self.pending.drain(..) { if output.abort().is_err() { failed = true; } }
        if failed { Err(CoreError::new(ErrorCode::NeedsAttention, "archive staging cleanup failed")) } else { Ok(()) }
    }
    fn flush_pending(&mut self, check: &mut impl FnMut(u64)->Result<()>) -> Result<()> {
        while let Some((output, file)) = self.pending.pop() {
            if let Err(error) = check(self.total) { output.abort()?; return Err(error); }
            // Record before publish: a lost commit reply may have created it.
            self.record.files.push(file);
            output.publish()?;
        }
        Ok(())
    }
    pub(crate) fn publish(mut self, check: &mut impl FnMut(u64)->Result<()>) -> Result<u64> {
        if let Err(error) = self.flush_pending(check) { self.abort()?; return Err(error); }
        Ok(self.total)
    }
}

pub struct SevenZipService<R: Read+Seek> { reader: SevenZReader<R>, entries: Vec<ArchiveEntry>, limits: CodecLimits, has_password: bool }
/// 7z AES-256 + SHA-256 coder.
const AES_CODER: [u8; 4] = [0x06, 0xF1, 0x07, 0x01];
impl<R: Read+Seek> SevenZipService<R> {
    pub fn open(mut source: R, limits: CodecLimits, password: Option<Zeroizing<String>>, mut check: impl FnMut()->Result<()>) -> Result<Self> {
        limits.validate()?; check()?;
        let length = source.seek(SeekFrom::End(0)).map_err(CoreError::io)?;
        if length > limits.archive.archive_bytes || length < 32 { return Err(large()); }
        source.seek(SeekFrom::Start(0)).map_err(CoreError::io)?;
        let mut signature = [0u8;32]; source.read_exact(&mut signature).map_err(CoreError::io)?;
        let header_size = u64::from_le_bytes(signature[20..28].try_into().map_err(|_| invalid())?);
        let header_offset = u64::from_le_bytes(signature[12..20].try_into().map_err(|_| invalid())?);
        if header_size > limits.archive.directory_bytes || header_offset.checked_add(32).and_then(|v|v.checked_add(header_size)).is_none_or(|v|v>length) { return Err(large()); }
        // Bound what the library will decode for the header itself (encoded
        // header size and dictionary) before handing the archive over.
        source.seek(SeekFrom::Start(32 + header_offset)).map_err(CoreError::io)?;
        let mut next = vec![0u8; header_size as usize];
        source.read_exact(&mut next).map_err(CoreError::io)?;
        if crc32(&next) != u32::from_le_bytes(signature[28..32].try_into().map_err(|_| invalid())?) { return Err(invalid()); }
        crate::sevenz_guard::check(&next, limits.archive.directory_bytes)?;
        drop(next);
        source.seek(SeekFrom::Start(0)).map_err(CoreError::io)?;
        if password.as_ref().is_some_and(|p| p.len()>4096) { return Err(invalid()); }
        let has_password = password.is_some();
        let secret = password.as_ref().map(|p| Password::from(p.as_str())).unwrap_or_default();
        let reader = SevenZReader::new(source, length, secret).map_err(codec)?;
        // Reject oversized payload dictionaries before allocating payload decoders.
        // Header decoding occurs inside the dependency and has a separate caveat.
        for folder in &reader.archive().folders {
            for coder in &folder.coders {
                let id=coder.decompression_method_id();
                let dictionary = if id==[0x21] && coder.properties.len()==1 {
                    let property=coder.properties[0];
                    if property>40 { return Err(invalid()); }
                    if property==40 { u32::MAX as u64 } else { (2u64 | (property as u64 & 1)) << (property as u32/2+11) }
                } else if id==[3,1,1] && coder.properties.len()==5 {
                    u32::from_le_bytes(coder.properties[1..5].try_into().map_err(|_|invalid())?) as u64
                } else { 0 };
                if dictionary>64*1024*1024 { return Err(large()); }
            }
        }
        let mut entries = Vec::new();
        if reader.archive().files.len() > limits.archive.entries { return Err(large()); }
        let archive = reader.archive();
        let encrypted_folder = |folder: Option<usize>| folder.and_then(|f| archive.folders.get(f))
            .is_some_and(|f| f.coders.iter().any(|c| c.decompression_method_id() == AES_CODER));
        for (position, file) in archive.files.iter().enumerate() {
            check()?;
            // Reject deletion markers, reparse points and Unix symlink attributes.
            if file.is_anti_item || (file.has_windows_attributes && (file.windows_attributes & 0x400 != 0
                || (file.windows_attributes >> 16) & 0o170000 == 0o120000)) { return Err(invalid()); }
            let encrypted = !file.is_directory && encrypted_folder(archive.stream_map.file_folder_index.get(position).copied().flatten());
            entries.push(ArchiveEntry { path: ResourcePath::parse(file.name.trim_end_matches('/'))?,
                directory: file.is_directory, bytes: file.size, compressed_bytes: file.compressed_size, encrypted });
        }
        let entries = index(entries, limits)?; check()?;
        Ok(Self { reader, entries, limits, has_password })
    }
    pub fn entries(&self) -> &[ArchiveEntry] { &self.entries }
    /// Before any decoding: encrypted content and no password.
    fn require_password(&self) -> Result<()> {
        if crate::any_encrypted(&self.entries) && !self.has_password { Err(crate::archive::password_required()) } else { Ok(()) }
    }
    /// A decoding failure of encrypted content with a password given is most
    /// likely the password (7z has no password check value).
    fn decode_failure(&self, otherwise: CoreError) -> CoreError {
        if crate::any_encrypted(&self.entries) && self.has_password { crate::archive::wrong_password() } else { otherwise }
    }
    /// Decode every entry once, in archive order: `f(index into entries(), stream)`.
    /// Used by the isolated worker; nothing is written here.
    pub(crate) fn for_each(&mut self, mut f: impl FnMut(usize, &mut dyn Read) -> Result<()>) -> Result<()> {
        self.require_password()?;
        let positions: BTreeMap<String, usize> = self.entries.iter().enumerate().map(|(i, e)| (e.path.as_str().to_owned(), i)).collect();
        let mut seen = BTreeSet::new();
        let mut failure = None;
        let result = self.reader.for_each_entries(|file, input| {
            let path = file.name.trim_end_matches('/');
            let Some(&index) = positions.get(path) else { return Err(sevenz_rust2::Error::other("unexpected entry")) };
            if !seen.insert(index) { return Err(sevenz_rust2::Error::other("duplicate entry")); }
            f(index, input).map_err(|e| { failure = Some(e); sevenz_rust2::Error::other("entry stopped") })?;
            Ok(true)
        });
        if let Some(error) = failure {
            return Err(if error.code == ErrorCode::InvalidInput { self.decode_failure(error) } else { error });
        }
        if result.is_err() || seen.len() != self.entries.len() { return Err(self.decode_failure(invalid())); }
        Ok(())
    }
    pub fn extract(&mut self, storage: &dyn Storage, target: &ResourcePath, check: impl FnMut(u64)->Result<()>) -> Result<u64> {
        self.extract_recorded(storage, target, check, &mut crate::extraction::Recorder::default())
    }
    pub(crate) fn extract_recorded(&mut self, storage: &dyn Storage, target: &ResourcePath, mut check: impl FnMut(u64)->Result<()>,
        record: &mut crate::extraction::Recorder) -> Result<u64> {
        self.extract_mode(storage, target, &mut check, record, false)
    }
    pub(crate) fn extract_private_recorded(&mut self, storage: &dyn Storage, target: &ResourcePath, mut check: impl FnMut(u64)->Result<()>,
        record: &mut crate::extraction::Recorder) -> Result<u64> {
        self.extract_mode(storage, target, &mut check, record, true)
    }
    fn extract_mode(&mut self, storage: &dyn Storage, target: &ResourcePath, check: &mut impl FnMut(u64)->Result<()>,
        record: &mut crate::extraction::Recorder, private_spool: bool) -> Result<u64> {
        check(0)?;
        self.require_password()?;
        let mut destination = if private_spool { Destination::private(storage,target,&self.entries,self.limits.pending_files,record)? }
            else { Destination::new(storage,target,&self.entries,self.limits.pending_files,record)? };
        let expected: BTreeMap<_,_> = self.entries.iter().map(|e| (e.path.as_str().to_owned(),e)).collect();
        let mut seen = BTreeSet::new();
        let result = self.reader.for_each_entries(|file,input| {
            let path = file.name.trim_end_matches('/');
            let entry = expected.get(path).ok_or_else(|| sevenz_rust2::Error::other("unexpected entry"))?;
            if !seen.insert(path.to_owned()) { return Err(sevenz_rust2::Error::other("duplicate entry")); }
            destination.push(entry,input,check).map_err(|_| sevenz_rust2::Error::other("entry extraction stopped"))?;
            Ok(true)
        });
        if result.is_err() || seen.len()!=self.entries.len() {
            destination.abort()?; return Err(self.decode_failure(partial()));
        }
        destination.publish(check).map_err(|_|partial())
    }
}
impl SevenZipService<std::io::Cursor<Vec<u8>>> {
    /// Non-solid LZMA2 creation into a new empty private seekable output.
    /// The finished archive is re-read and must list exactly the requested
    /// entries with no anti-items before success is reported.
    pub fn create<W: Read+Write+Seek>(output: &mut W, sources: Vec<ZipSource>, limits: CodecLimits,
        mut check: impl FnMut(u64)->Result<()>) -> Result<()> {
        limits.validate()?;
        let entries = index(sources.iter().map(|s| ArchiveEntry { path:s.path.clone(), directory:s.reader.is_none(), bytes:s.bytes, compressed_bytes:0, encrypted:false }),limits)?;
        if output.stream_position().map_err(CoreError::io)? != 0 { return Err(invalid()); }
        let mut writer = SevenZWriter::new(crate::archive::LimitedOutput {inner:output,position:0,limit:limits.archive.archive_bytes}).map_err(codec)?;
        let mut total = 0u64;
        let requested = entries.clone();
        for (source,entry) in sources.into_iter().zip(entries) {
            check(total)?;
            let metadata = if entry.directory {
                // sevenz-rust2 0.7.0 writes the kAnti bit for stream-less entries
                // whose flag is false (inverted). Setting it here yields no kAnti
                // property; verify_created() fails closed if that ever changes.
                let mut folder = SevenZArchiveEntry::new_folder(entry.path.as_str()); folder.is_anti_item = true; folder
            } else { SevenZArchiveEntry::new_file(entry.path.as_str()) };
            let reader = source.reader.map(|input| CheckedInput {input,bytes:0,expected:entry.bytes,check:&mut check,base:total});
            writer.push_archive_entry(metadata,reader).map_err(codec)?;
            total = total.checked_add(entry.bytes).ok_or_else(large)?;
        }
        check(total)?; writer.finish().map_err(CoreError::io)?.flush().map_err(CoreError::io)?;
        verify_created(output, &requested, limits)
    }
}
fn verify_created<W: Read+Seek>(output: &mut W, requested: &[ArchiveEntry], limits: CodecLimits) -> Result<()> {
    let failed = || CoreError::new(ErrorCode::Integrity, "created 7z archive does not match request");
    let length = output.seek(SeekFrom::End(0)).map_err(CoreError::io)?;
    output.seek(SeekFrom::Start(0)).map_err(CoreError::io)?;
    let reader = SevenZReader::new(&mut *output, length, Password::default()).map_err(|_| failed())?;
    let files = &reader.archive().files;
    if files.len() != requested.len() || files.len() > limits.archive.entries { return Err(failed()); }
    for (file, entry) in files.iter().zip(requested) {
        if file.is_anti_item || file.is_directory != entry.directory || file.size != entry.bytes
            || file.name.trim_end_matches('/') != entry.path.as_str() { return Err(failed()); }
    }
    drop(reader);
    output.seek(SeekFrom::End(0)).map_err(CoreError::io)?; Ok(())
}
struct CheckedInput<'a,F> { input: Box<dyn Read>,bytes:u64,expected:u64,check:&'a mut F,base:u64 }
impl<F:FnMut(u64)->Result<()>> Read for CheckedInput<'_,F> {
    fn read(&mut self,b:&mut[u8])->std::io::Result<usize> {
        (self.check)(self.base+self.bytes).map_err(|_|std::io::Error::other("cancelled"))?;
        let n=self.input.read(b)?;
        self.bytes=self.bytes.checked_add(n as u64).ok_or_else(||std::io::Error::other("size overflow"))?;
        if self.bytes>self.expected || (n==0 && self.bytes!=self.expected) { return Err(std::io::Error::other("source changed")); } Ok(n)
    }
}
fn codec(error: sevenz_rust2::Error)->CoreError {
    match error {
        sevenz_rust2::Error::PasswordRequired => crate::archive::password_required(),
        sevenz_rust2::Error::MaybeBadPassword(_) => crate::archive::wrong_password(),
        _ => invalid(),
    }
}
fn invalid()->CoreError { CoreError::new(ErrorCode::InvalidInput,"archive codec or layout rejected") }
fn large()->CoreError { CoreError::new(ErrorCode::LimitExceeded,"archive codec limit exceeded") }
fn partial()->CoreError { CoreError::new(ErrorCode::NeedsAttention,"archive extraction incomplete; inspect retained destination") }

/// CRC-32 (IEEE) for the 7z next-header check, without another dependency.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 { crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 }; }
    }
    !crc
}
