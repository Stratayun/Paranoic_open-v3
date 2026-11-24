//! Prepare ciphertext for journaled import; never publish into a live vault.
use std::{io::{Read, Write}, cell::RefCell, rc::Rc};
use paranoic_core::ResourcePath;
use paranoic_crypto::{CryptoError, Result, StreamSink};
use crate::{CryptomatorReader, CryptomatorContentLimits};

/// A completed ciphertext artifact, not a successful vault write receipt.
/// No plaintext names or keys are retained. Fields cannot be forged externally.
pub struct CryptomatorPreparedImport {
    storage_id: paranoic_core::StorageId,
    binding: String,
    vault_id: uuid::Uuid,
    entry_path: ResourcePath,
    content_path: ResourcePath,
    mapping: Option<String>,
    plaintext_bytes: u64,
    ciphertext_bytes: u64,
    digest: [u8; 32],
}

impl CryptomatorPreparedImport {
    pub fn vault_id(&self) -> uuid::Uuid { self.vault_id }
    /// The final namespace entry. For c9s this must be an atomic wrapper publish.
    pub fn entry_path(&self) -> &ResourcePath { &self.entry_path }
    pub fn content_path(&self) -> &ResourcePath { &self.content_path }
    /// Write as entry_path/name.c9s before publishing a shortened wrapper.
    pub fn name_mapping(&self) -> Option<&str> { self.mapping.as_deref() }
    pub fn plaintext_bytes(&self) -> u64 { self.plaintext_bytes }
    pub fn ciphertext_bytes(&self) -> u64 { self.ciphertext_bytes }
    pub fn ciphertext_blake3(&self) -> &[u8; 32] { &self.digest }

    /// Independent full readback of the prepared ciphertext or destination.
    /// This only verifies bytes at the time of reading. It does not authorize
    /// overwrite, prove durable publication, or pin a version for later use.
    pub fn verify_ciphertext<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, mut input: R, mut progress: F,
    ) -> Result<()> {
        let mut hasher = blake3::Hasher::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            progress(bytes)?;
            let remaining = self.ciphertext_bytes - bytes;
            let capacity = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
            let count = match input.read(&mut buffer[..capacity]) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if count == 0 { break; }
            if count as u64 > remaining { return Err(CryptoError::Authentication); }
            hasher.update(&buffer[..count]);
            bytes += count as u64;
        }
        if bytes != self.ciphertext_bytes || hasher.finalize() != blake3::Hash::from(self.digest) {
            return Err(CryptoError::Authentication);
        }
        progress(bytes)
    }
}

impl CryptomatorReader {
    /// Queue a create-new file import through the durable core journal.
    /// The caller persists job_id before submission and reuses it only for this
    /// exact request after an uncertain acknowledgement. The source is retained.
    /// c9s requires a backend supporting atomic wrapper publication.
    pub fn submit_file_import(
        &self, core: &paranoic_core::Core, prepared: &CryptomatorPreparedImport,
        ciphertext_source: paranoic_core::Location, job_id: paranoic_core::JobId,
    ) -> paranoic_core::Result<paranoic_core::JobRecord> {
        use paranoic_core::{CoreError, ErrorCode, Location, Operation};
        let id = self.session.id().map_err(|_| CoreError::new(ErrorCode::PermissionDenied, "vault session locked"))?;
        if id != prepared.vault_id || self.storage.id() != prepared.storage_id {
            return Err(CoreError::new(ErrorCode::InvalidInput, "prepared import belongs to another vault"));
        }
        let binding = self.storage.binding_key()?;
        if blake3::hash(binding.as_bytes()).to_hex().as_str() != prepared.binding
            || core.storage_binding_fingerprint(prepared.storage_id)? != prepared.binding
        { return Err(CoreError::new(ErrorCode::PermissionDenied, "vault storage binding changed")); }
        let operation = if let Some(mapping) = &prepared.mapping {
            Operation::CopyExpectedBundle {
                source: ciphertext_source,
                target: Location { storage: prepared.storage_id, path: prepared.entry_path.clone() },
                bytes: prepared.ciphertext_bytes, blake3: prepared.digest,
                content_name: "contents.c9r".to_owned(), sidecar_name: "name.c9s".to_owned(),
                sidecar: mapping.as_bytes().to_vec(),
            }
        } else {
            Operation::CopyExpected {
                source: ciphertext_source,
                target: Location { storage: prepared.storage_id, path: prepared.content_path.clone() },
                bytes: prepared.ciphertext_bytes, blake3: prepared.digest,
            }
        };
        core.submit_with_id(job_id, operation)
    }

    /// Encrypt a new file into a HOST-PRIVATE ciphertext spool. The supplied
    /// sink must not target the live vault entry. On success its commit has
    /// published only the spool artifact. Persist a journal intent before
    /// subsequently attempting any vault mutation; retain plaintext source.
    pub fn prepare_file_import<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, parent: &[String], name: &str, input: R, output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, mut progress: F,
    ) -> Result<CryptomatorPreparedImport> {
        let destination: Result<_> = (|| {
            let vault_id = self.session.id()?;
            let (directory, path) = self.resolve_directory(parent, &mut || {
                self.session.id()?;
                progress(0)
            })?;
            let entry = self.session.encrypt_name(&directory, name)?;
            let entry_path = path.join(entry.component()).map_err(|_| CryptoError::InvalidInput)?;
            let mapping = entry.name_mapping().map(str::to_owned);
            let content_path = if mapping.is_some() {
                entry_path.join("contents.c9r").map_err(|_| CryptoError::InvalidInput)?
            } else { entry_path.clone() };
            let binding = self.storage.binding_key().map_err(|_| CryptoError::Io)?;
            Ok((vault_id, entry_path, content_path, mapping, blake3::hash(binding.as_bytes()).to_hex().to_string()))
        })();
        let (vault_id, entry_path, content_path, mapping, binding) = match destination {
            Ok(value) => value,
            Err(error) => {
                output.abort().map_err(|_| CryptoError::CleanupFailed)?;
                return Err(error);
            }
        };
        let receipt = Rc::new(RefCell::new(None));
        let sink = HashingSink { output, hasher: blake3::Hasher::new(), bytes: 0, receipt: receipt.clone() };
        let plaintext_bytes = self.session.encrypt_content(input, Box::new(sink), limits, progress)?;
        let (ciphertext_bytes, digest) = receipt.borrow_mut().take().ok_or(CryptoError::PublicationUncertain)?;
        Ok(CryptomatorPreparedImport { storage_id: self.storage.id(), binding, vault_id, entry_path, content_path, mapping,
            plaintext_bytes, ciphertext_bytes, digest })
    }
}

struct HashingSink {
    output: Box<dyn StreamSink>,
    hasher: blake3::Hasher,
    bytes: u64,
    #[allow(clippy::type_complexity)]
    receipt: Rc<RefCell<Option<(u64, [u8; 32])>>>,
}
impl Write for HashingSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.output.write(bytes)?;
        if count > bytes.len() { return Err(std::io::Error::other("invalid write count")); }
        self.bytes = self.bytes.checked_add(count as u64).ok_or_else(|| std::io::Error::other("size overflow"))?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.output.flush() }
}
impl StreamSink for HashingSink {
    fn commit(self: Box<Self>) -> Result<()> {
        let Self { output, hasher, bytes, receipt } = *self;
        output.commit()?;
        *receipt.borrow_mut() = Some((bytes, *hasher.finalize().as_bytes()));
        Ok(())
    }
    fn abort(self: Box<Self>) -> Result<()> { self.output.abort() }
}
