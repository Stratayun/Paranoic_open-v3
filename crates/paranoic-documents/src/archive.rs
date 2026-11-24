use std::{collections::{BTreeMap, BTreeSet}, io::{Read, Write, Seek, SeekFrom}};
use paranoic_core::{Storage, StorageDomain, ResourcePath, JobId};
use zeroize::Zeroizing;
use zip::{ZipArchive, ZipWriter, CompressionMethod, AesMode, write::SimpleFileOptions};
use crate::{CoreError, ErrorCode, Result};

#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub archive_bytes: u64, pub entries: usize, pub entry_bytes: u64,
    pub total_bytes: u64, pub directory_bytes: u64, pub ratio: u64, pub depth: usize,
}
impl Default for ArchiveLimits {
    fn default() -> Self { Self { archive_bytes: 1024 * 1024 * 1024 * 1024, entries: 10000,
        entry_bytes: 16 * 1024 * 1024 * 1024, total_bytes: 64 * 1024 * 1024 * 1024, directory_bytes: 32 * 1024 * 1024, ratio: 1000, depth: 64 } }
}
impl ArchiveLimits {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.archive_bytes == 0 || self.entries == 0 || self.entries > 100000 || self.entry_bytes == 0
            || self.total_bytes == 0 || self.directory_bytes == 0 || self.directory_bytes > 128 * 1024 * 1024 || self.ratio == 0 || self.depth == 0 || self.depth > 128 { return Err(invalid()); }
        Ok(())
    }
}
/// `encrypted`: the entry's content needs the archive password (names and
/// sizes are readable without it).
#[derive(Clone, Debug)]
pub struct ArchiveEntry { pub path: ResourcePath, pub directory: bool, pub bytes: u64, pub compressed_bytes: u64, pub encrypted: bool }

/// Messages of the two password outcomes (code PermissionDenied), so a host
/// can ask for the password instead of reporting damaged data (E2.1).
pub const PASSWORD_REQUIRED: &str = "archive password required";
pub const WRONG_PASSWORD: &str = "wrong archive password or damaged encrypted data";
pub(crate) fn password_required() -> CoreError { CoreError::new(ErrorCode::PermissionDenied, PASSWORD_REQUIRED) }
pub(crate) fn wrong_password() -> CoreError { CoreError::new(ErrorCode::PermissionDenied, WRONG_PASSWORD) }
pub fn is_password_required(error: &CoreError) -> bool { error.code == ErrorCode::PermissionDenied && error.message == PASSWORD_REQUIRED }
pub fn is_wrong_password(error: &CoreError) -> bool { error.code == ErrorCode::PermissionDenied && error.message == WRONG_PASSWORD }
/// True if any entry needs the password.
pub fn any_encrypted(entries: &[ArchiveEntry]) -> bool { entries.iter().any(|e| e.encrypted) }
/// None denotes an empty directory. File streams are consumed exactly once.
pub struct ZipSource { pub path: ResourcePath, pub bytes: u64, pub reader: Option<Box<dyn Read>> }
pub struct ZipService<R: Read + Seek> { archive: ZipArchive<R>, entries: Vec<ArchiveEntry>, limits: ArchiveLimits }

impl<R: Read + Seek> ZipService<R> {
    pub fn open(mut input: R, limits: ArchiveLimits, mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        limits.validate()?; check()?;
        crate::zip_guard::inspect(&mut input, limits, &mut check)?;
        let mut archive = ZipArchive::new(input).map_err(zip_error)?;
        if archive.len() > limits.entries { return Err(large()); }
        let mut entries = Vec::with_capacity(archive.len());
        let mut names = BTreeMap::new(); let mut total = 0u64;
        for index in 0..archive.len() {
            check()?;
            let file = archive.by_index_raw(index).map_err(zip_error)?;
            let raw = std::str::from_utf8(file.name_raw()).map_err(|_| invalid())?;
            let directory = raw.ends_with('/');
            let name = if directory { &raw[..raw.len()-1] } else { raw };
            let path = ResourcePath::parse(name)?;
            validate_name(&mut names, &path, directory, limits.depth)?;
            if let Some(mode) = file.unix_mode() {
                let kind = mode & 0o170000;
                if kind != 0 && kind != if directory { 0o040000 } else { 0o100000 } { return Err(invalid()); }
            }
            let bytes = file.size(); let compressed_bytes = file.compressed_size();
            if (directory && bytes != 0) || bytes > limits.entry_bytes
                || bytes > compressed_bytes.max(1).saturating_mul(limits.ratio) { return Err(large()); }
            total = total.checked_add(bytes).ok_or_else(large)?;
            if total > limits.total_bytes { return Err(large()); }
            entries.push(ArchiveEntry { path, directory, bytes, compressed_bytes, encrypted: !directory && file.encrypted() });
        }
        check()?; Ok(Self { archive, entries, limits })
    }
    pub fn entries(&self) -> &[ArchiveEntry] { &self.entries }

    /// Plan 4.7: every file entry in order, as a stream of exactly its
    /// declared size (the CRC/authentication is checked at its end). Nothing
    /// is written here — the caller encrypts straight into a vault. Password
    /// problems are reported before the first entry, as for extraction.
    pub fn for_each_file(&mut self, password: Option<Zeroizing<String>>,
        mut f: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<()>) -> Result<()> {
        if password.as_ref().is_some_and(|p| p.len() > 4096) { return Err(invalid()); }
        if let Some(first) = self.entries.iter().position(|e| e.encrypted) {
            let secret = password.as_ref().ok_or_else(password_required)?;
            match self.archive.by_index_decrypt(first, secret.as_bytes()) {
                Ok(_) => {}
                Err(zip::result::ZipError::InvalidPassword) => return Err(wrong_password()),
                Err(error) => return Err(zip_error(error)),
            }
        }
        let entries = self.entries.clone();
        for (index, entry) in entries.iter().enumerate() {
            if entry.directory { continue; }
            let mut input = match (&password, entry.encrypted) {
                (Some(secret), true) => self.archive.by_index_decrypt(index, secret.as_bytes()).map_err(zip_error)?,
                _ => self.archive.by_index(index).map_err(zip_error)?,
            };
            let mut exact = ExactRead { inner: &mut input, left: entry.bytes };
            f(entry, &mut exact)?;
            // Read to the end: the codec checks the CRC there; nothing may follow.
            let mut rest = [0u8; 1];
            if exact.left != 0 || input.read(&mut rest).map_err(|_| invalid())? != 0 { return Err(invalid()); }
        }
        Ok(())
    }

    /// Create a new destination root; never merge into an existing tree.
    /// Completed files survive interruption. No recursive rollback or replay.
    pub fn extract(&mut self, storage: &dyn Storage, target: &ResourcePath,
        password: Option<Zeroizing<String>>, check: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        self.extract_recorded(storage, target, password, check, &mut crate::extraction::Recorder::default())
    }
    pub(crate) fn extract_recorded(&mut self, storage: &dyn Storage, target: &ResourcePath,
        password: Option<Zeroizing<String>>, mut check: impl FnMut(u64) -> Result<()>,
        record: &mut crate::extraction::Recorder) -> Result<u64> {
        if target.is_root() || storage.domain() != StorageDomain::Files || !storage.capabilities().write { return Err(invalid()); }
        if password.as_ref().is_some_and(|p| p.len() > 4096) { return Err(invalid()); }
        // Password problems are found before the first mutation: missing, or
        // rejected by the first encrypted entry's check value.
        if let Some(first) = self.entries.iter().position(|e| e.encrypted) {
            let secret = password.as_ref().ok_or_else(password_required)?;
            match self.archive.by_index_decrypt(first, secret.as_bytes()) {
                Ok(_) => {}
                Err(zip::result::ZipError::InvalidPassword) => return Err(wrong_password()),
                Err(error) => return Err(zip_error(error)),
            }
        }
        // Resolve every final path before first mutation.
        let paths = self.entries.iter().map(|e| ResourcePath::parse(format!("{}/{}", target.as_str(), e.path.as_str())))
            .collect::<Result<Vec<_>>>()?;
        check(0)?; storage.create_directory(target)?;
        record.directories.push(ResourcePath::root());
        let result = (|| {
            let mut directories = BTreeSet::new(); directories.insert(target.as_str().to_owned());
            let mut total = 0u64; let mut buffer = Zeroizing::new(vec![0u8; 65536]);
            for (index, entry) in self.entries.iter().enumerate() {
                check(total)?;
                let path = &paths[index];
                let needed = if entry.directory { path.clone() } else { path.parent_and_name()?.0 };
                let suffix = needed.as_str().strip_prefix(target.as_str()).ok_or_else(invalid)?;
                let mut current = target.clone();
                for component in suffix.trim_start_matches('/').split('/').filter(|s| !s.is_empty()) {
                    current = current.join(component)?;
                    if directories.insert(current.as_str().to_owned()) {
                        storage.create_directory(&current)?;
                        record.directories.push(ResourcePath::parse(current.as_str()[target.as_str().len()+1..].to_owned())?);
                    }
                }
                if entry.directory { continue; }
                let mut input = match &password {
                    Some(secret) => self.archive.by_index_decrypt(index, secret.as_bytes()).map_err(zip_error)?,
                    None => self.archive.by_index(index).map_err(zip_error)?,
                };
                let mut output = storage.stage(path, JobId(uuid::Uuid::new_v4()))?;
                let mut hash = blake3::Hasher::new();
                let write = (|| {
                    let mut bytes = 0u64;
                    loop {
                        check(total)?;
                        let n = input.read(&mut buffer).map_err(CoreError::io)?;
                        if n == 0 { break; }
                        bytes = bytes.checked_add(n as u64).ok_or_else(large)?;
                        total = total.checked_add(n as u64).ok_or_else(large)?;
                        if bytes > entry.bytes || bytes > self.limits.entry_bytes || total > self.limits.total_bytes { return Err(large()); }
                        output.write_all(&buffer[..n]).map_err(CoreError::io)?;
                        hash.update(&buffer[..n]);
                    }
                    if bytes != entry.bytes { return Err(invalid()); }
                    output.flush().map_err(CoreError::io)?; check(total)
                })();
                if let Err(error) = write { output.abort()?; return Err(error); }
                output.publish()?;
                record.files.push(crate::ExtractedFile { path: entry.path.clone(), bytes: entry.bytes, blake3: *hash.finalize().as_bytes() });
            }
            check(total)?; Ok(total)
        })();
        result.map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "archive extraction incomplete; retain destination for review"))
    }
}

/// Write into a host-owned tentative seekable output. Return it only after the
/// ZIP central directory is finished. Caller publishes on success, aborts on error.
impl ZipService<std::io::Cursor<Vec<u8>>> {
    pub fn create<W: Write + Seek>(output: &mut W, sources: Vec<ZipSource>, limits: ArchiveLimits,
        password: Option<Zeroizing<String>>, mut check: impl FnMut(u64) -> Result<()>) -> Result<()> {
        limits.validate()?;
        if sources.len() > limits.entries || password.as_ref().is_some_and(|p| p.is_empty() || p.len() > 4096) { return Err(invalid()); }
        let mut names = BTreeMap::new(); let mut expected = 0u64;
        let mut directory_bytes = 0u64;
        for source in &sources {
            validate_name(&mut names, &source.path, source.reader.is_none(), limits.depth)?;
            // Conservative bound for our own fixed ZIP64/AES extra fields.
            directory_bytes = directory_bytes.checked_add(source.path.as_str().len() as u64 + 256).ok_or_else(large)?;
            if directory_bytes > limits.directory_bytes { return Err(large()); }
            if source.bytes > limits.entry_bytes || (source.reader.is_none() && source.bytes != 0) { return Err(large()); }
            expected = expected.checked_add(source.bytes).ok_or_else(large)?;
            if expected > limits.total_bytes { return Err(large()); }
        }
        check(0)?;
        if output.stream_position().map_err(CoreError::io)? != 0 { return Err(invalid()); }
        let mut writer = ZipWriter::new(LimitedOutput { inner: output, position: 0, limit: limits.archive_bytes });
        let mut total = 0u64; let mut buffer = Zeroizing::new(vec![0u8; 65536]);
        for source in sources {
            check(total)?;
            let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(true).unix_permissions(0o600);
            if let Some(secret) = &password { options = options.with_aes_encryption(AesMode::Aes256, secret); }
            if let Some(mut reader) = source.reader {
                writer.start_file(source.path.as_str(), options).map_err(zip_error)?;
                let mut bytes = 0u64;
                loop {
                    check(total)?;
                    let n = reader.read(&mut buffer).map_err(CoreError::io)?;
                    if n == 0 { break; }
                    bytes = bytes.checked_add(n as u64).ok_or_else(large)?;
                    if bytes > source.bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "archive input grew")); }
                    writer.write_all(&buffer[..n]).map_err(CoreError::io)?; total += n as u64;
                }
                if bytes != source.bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "archive input shrank")); }
            } else { writer.add_directory(format!("{}/", source.path.as_str()), SimpleFileOptions::default().unix_permissions(0o700)).map_err(zip_error)?; }
        }
        check(total)?;
        writer.finish().map_err(zip_error)?.flush().map_err(CoreError::io)?;
        Ok(())
    }
}
pub(crate) fn validate_name(names: &mut BTreeMap<String, bool>, path: &ResourcePath, directory: bool, depth: usize) -> Result<()> {
    if path.is_root() || path.as_str().split('/').count() > depth { return Err(invalid()); }
    let key = path.as_str().to_lowercase();
    if names.contains_key(&key) { return Err(invalid()); }
    let mut parent = key.as_str();
    while let Some((prefix, _)) = parent.rsplit_once('/') {
        if names.get(prefix) == Some(&false) { return Err(invalid()); }
        parent = prefix;
    }
    if !directory {
        let prefix = format!("{key}/");
        if names.range(prefix.clone()..).next().is_some_and(|(name, _)| name.starts_with(&prefix)) { return Err(invalid()); }
    }
    names.insert(key, directory); Ok(())
}
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid archive structure or request") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "archive exceeds configured limits") }
fn zip_error(_: zip::result::ZipError) -> CoreError { CoreError::new(ErrorCode::InvalidInput, "archive codec rejected input or output") }

pub(crate) struct LimitedOutput<'a, W> { pub(crate) inner: &'a mut W, pub(crate) position: u64, pub(crate) limit: u64 }
impl<W: Write> Write for LimitedOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.position) {
            return Err(std::io::Error::other("archive output limit exceeded"));
        }
        let n = self.inner.write(bytes)?;
        if n > bytes.len() { return Err(std::io::Error::other("invalid write count")); }
        self.position += n as u64; Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}
impl<W: Seek> Seek for LimitedOutput<'_, W> {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let position = self.inner.seek(from)?;
        self.position = position;
        if position > self.limit { return Err(std::io::Error::other("archive seek limit exceeded")); }
        Ok(position)
    }
}

/// Never more than the declared size from an entry stream.
pub(crate) struct ExactRead<'a, R: Read> { pub(crate) inner: &'a mut R, pub(crate) left: u64 }
impl<R: Read> Read for ExactRead<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || out.is_empty() { return Ok(0); }
        let want = out.len().min(self.left.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut out[..want])?;
        self.left -= n as u64;
        Ok(n)
    }
}
