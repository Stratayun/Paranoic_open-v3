//! Work inside an open Cryptomator vault without plaintext files (E4.7):
//! random-access streams (read with `vault_stream_read`/`vault_stream_close`),
//! text open/save/create with a strong version token, archives listed and
//! extracted into a new folder of the same vault. Same contracts as the
//! 1.8 calls in vault_io.rs. Format limitation: Cryptomator does not
//! authenticate the number of chunks of a file (removal of whole trailing
//! chunks is not detectable); everything read is authenticated.
use crate::*;
use crate::crypto_files::crypto_error;
use crate::features::StageSink;
use crate::vault_io::{hex, register_stream, MemorySink, VaultKind, VaultSeek};
use paranoic_documents::TextDocument;
use paranoic_formats::CryptomatorLengthPolicy;
use std::io::Read;

impl NativeRuntime {
    fn cm_file(path: &[String], name: &str) -> Vec<String> { let mut f = path.to_vec(); f.push(name.to_owned()); f }
    fn cm_seek(&self, handle: &str, path: &[String], name: &str) -> Result<VaultSeek> {
        let reader = self.cryptomator(handle)?;
        let file = reader.open_file_reader(&Self::cm_file(path, name)).map_err(crypto_error)?;
        Ok(VaultSeek { reader: Box::new(file), position: 0 })
    }
    /// Encrypt `input` (exactly `size` bytes) as `name` in folder `path`: new,
    /// or replacing only the exact ciphertext `expected` (strong version).
    fn cm_put(&self, handle: &str, path: &[String], name: &str, input: &mut dyn Read,
        expected: Option<(Location, paranoic_core::Metadata, [u8; 32])>) -> Result<()> {
        let reader = self.cryptomator(handle)?;
        let spool = Self::spool_path()?;
        let staged = self.app.stage(&spool, JobId(Uuid::new_v4()))?;
        let prepared = reader.prepare_file_import(path, name, input, Box::new(StageSink(Some(staged))), Default::default(), |_| Ok(()))
            .map_err(crypto_error);
        let result = prepared.and_then(|prepared| {
            let source = Location { storage: self.app.id(), path: spool.clone() };
            let job = match expected {
                Some(expected) => reader.submit_file_replace(&self.core, &prepared, source, expected, JobId(Uuid::new_v4()))?,
                None => reader.submit_file_import(&self.core, &prepared, source, JobId(Uuid::new_v4()))?,
            };
            features::finish(features::run_wait(&self.core, job.id)?, 1)
        });
        self.drop_spool(&spool);
        result
    }
    pub(crate) fn cm_bytes(&self, handle: &str, path: &[String], name: &str, limit: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let reader = self.cryptomator(handle)?;
        let buffer = std::rc::Rc::new(std::cell::RefCell::new(zeroize::Zeroizing::new(Vec::new())));
        reader.decrypt_file(&Self::cm_file(path, name), Box::new(MemorySink(Some(buffer.clone()), limit)), Default::default(),
            CryptomatorLengthPolicy::AllowUnauthenticated, |_| Ok(())).map_err(crypto_error)?;
        let bytes = std::mem::take(&mut *buffer.borrow_mut());
        Ok(bytes)
    }
    fn cm_version(&self, handle: &str, path: &[String], name: &str) -> Result<(Location, paranoic_core::Metadata, [u8; 32])> {
        self.cryptomator(handle)?.content_version(&Self::cm_file(path, name)).map_err(crypto_error)
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Open a Cryptomator file for random-access reading in memory; read and
    /// close with `vault_stream_read` / `vault_stream_close` (≤ 8 streams in
    /// all; locking the vault or Quick Lock ends them).
    pub fn cryptomator_stream_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeVaultStream> {
        let seek = self.cm_seek(&handle, &path, &name)?;
        register_stream(handle, VaultKind::Cryptomator, seek.reader)
    }
    /// A text file in a Cryptomator vault, decrypted in memory (≤ 16 MiB);
    /// `version_token` as for `vault_text_open`.
    pub fn cryptomator_text_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeVaultText> {
        let (location, version, digest) = self.cm_version(&handle, &path, &name)?;
        let bytes = self.cm_bytes(&handle, &path, &name, crate::documents::TEXT_LIMIT)?;
        let document = TextDocument::open(&bytes[..], crate::documents::TEXT_LIMIT, || Ok(()))?;
        if self.cm_version(&handle, &path, &name)? != (location, version.clone(), digest) { return Err(BridgeError::Changed); }
        Ok(NativeVaultText { text: document.text().to_owned(), encoding: crate::documents::encoding_code(document.encoding()),
            version_bytes: version.bytes, version_modified_ms: version.modified_unix_ms, version_token: hex(&digest) })
    }
    /// Save over the file, keeping its encoding — only over the exact
    /// ciphertext `version_token` (else `Changed`); Core rechecks it at replace.
    pub fn cryptomator_text_save(&self, handle: String, path: Vec<String>, name: String, version_token: String, text: String) -> Result<()> {
        let text = zeroize::Zeroizing::new(text);
        let current = self.cm_version(&handle, &path, &name)?;
        if hex(&current.2) != version_token { return Err(BridgeError::Changed); }
        let original = self.cm_bytes(&handle, &path, &name, crate::documents::TEXT_LIMIT)?;
        if self.cm_version(&handle, &path, &name)? != current { return Err(BridgeError::Changed); }
        let mut document = TextDocument::open(&original[..], crate::documents::TEXT_LIMIT, || Ok(()))?;
        let length = document.text().len();
        document.replace(document.revision(), 0..length, &text)?;
        let encoded = std::rc::Rc::new(std::cell::RefCell::new(zeroize::Zeroizing::new(Vec::new())));
        document.save(Box::new(MemorySink(Some(encoded.clone()), crate::documents::TEXT_LIMIT + 4)), || Ok(()))?;
        let bytes = std::mem::take(&mut *encoded.borrow_mut());
        self.cm_put(&handle, &path, &name, &mut &bytes[..], Some(current))
    }
    /// A NEW UTF-8 text file in a Cryptomator vault (`Conflict` if taken).
    pub fn cryptomator_text_create(&self, handle: String, path: Vec<String>, name: String, text: String) -> Result<()> {
        let text = zeroize::Zeroizing::new(text);
        if text.len() > crate::documents::TEXT_LIMIT || text.contains('\0') { return Err(BridgeError::Invalid); }
        if self.cryptomator_list(handle.clone(), path.clone())?.entries.iter().any(|e| e.name == name) { return Err(BridgeError::Conflict); }
        self.cm_put(&handle, &path, &name, &mut text.as_bytes(), None)
    }
    /// Entries of a ZIP/7z stored in a Cryptomator vault (as `vault_archive_list`).
    pub fn cryptomator_archive_list(&self, handle: String, path: Vec<String>, name: String, kind: u8, password: Option<String>) -> Result<Vec<NativeArchiveEntry>> {
        self.archive_list_from(self.cm_seek(&handle, &path, &name)?, kind, password)
    }
    /// Extract into a NEW folder of the same Cryptomator vault; every file is
    /// encrypted straight from the archive stream (as `vault_archive_extract`).
    #[allow(clippy::too_many_arguments)] // flat UniFFI call
    pub fn cryptomator_archive_extract(&self, handle: String, path: Vec<String>, name: String, kind: u8, password: Option<String>,
        target_path: Vec<String>, target_name: String) -> Result<u64> {
        let source = self.cm_seek(&handle, &path, &name)?;
        let mkdir = |parent: &[String], name: &str| self.cryptomator_mkdir(handle.clone(), parent.to_vec(), name.to_owned());
        let add = |parent: &[String], name: &str, _size: u64, input: &mut dyn Read| self.cm_put(&handle, parent, name, input, None);
        self.archive_extract_into(source, kind, password, target_path, target_name, &mkdir, &add)
    }
}
