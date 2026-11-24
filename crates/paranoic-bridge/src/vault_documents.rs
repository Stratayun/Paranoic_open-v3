//! PDF and Office documents viewed from inside an encrypted vault (E4.7):
//! the file is decrypted and authenticated in memory only; PDF bytes go to
//! the isolated PDF worker over its pipe (as for ordinary files), Office is
//! parsed in memory. No plaintext file, OS grant or public copy is made.
//! Locking the vault (or Quick Lock) ends its PDF session.
use crate::*;
use crate::vault_io::VaultKind;
use paranoic_documents::pdf_client::{PdfSession, PDF_INPUT_LIMIT};
use paranoic_documents::{open_office, OfficeLimits};

/// What the current PDF session was opened from.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum PdfKey {
    Plain(Location),
    Vault { kind: VaultKind, handle: String, file: Vec<String>, object: paranoic_core::Metadata },
}

impl NativeRuntime {
    fn vault_object_metadata(&self, kind: VaultKind, handle: &str, path: &[String], name: &str) -> Result<paranoic_core::Metadata> {
        match kind {
            VaultKind::Paranoic => self.vault_stat(handle, path, name),
            VaultKind::Cryptomator => {
                let mut file = path.to_vec(); file.push(name.to_owned());
                self.cryptomator(handle)?.content_metadata(&file).map_err(crate::crypto_files::crypto_error)
            }
        }
    }
    fn vault_plaintext(&self, kind: VaultKind, handle: &str, path: &[String], name: &str, limit: usize) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        match kind {
            VaultKind::Paranoic => self.vault_bytes(handle, path, name, limit),
            VaultKind::Cryptomator => self.cm_bytes(handle, path, name, limit),
        }
    }
    /// Reuse the worker only for the same, unchanged vault file.
    fn with_vault_pdf<T>(&self, kind: VaultKind, handle: &str, path: &[String], name: &str,
        f: impl FnOnce(&mut PdfSession) -> paranoic_core::Result<T>) -> Result<T> {
        let object = self.vault_object_metadata(kind, handle, path, name)?;
        let mut file = path.to_vec(); file.push(name.to_owned());
        let key = PdfKey::Vault { kind, handle: handle.to_owned(), file, object: object.clone() };
        let mut slot = self.pdf.lock().map_err(lock_error)?;
        let reusable = slot.as_ref().is_some_and(|(k, s)| *k == key && s.is_alive());
        if !reusable {
            slot.take();
            let data = self.vault_plaintext(kind, handle, path, name, PDF_INPUT_LIMIT as usize)?;
            // The bytes must belong to the version the key names.
            if self.vault_object_metadata(kind, handle, path, name)? != object { return Err(BridgeError::Changed); }
            let session = PdfSession::open_bytes_with_host(data, object, &self.pdf_worker, &self.pdfium, self.document_worker_host()?)?;
            *slot = Some((key, session));
        }
        let (_, session) = slot.as_mut().ok_or(BridgeError::Unavailable)?;
        Ok(f(session)?)
    }
    fn vault_office(&self, kind: VaultKind, handle: &str, path: &[String], name: &str) -> Result<NativeOfficeView> {
        let limits = if cfg!(target_os = "android") { OfficeLimits::phone() } else { OfficeLimits::default() };
        let bytes = self.vault_plaintext(kind, handle, path, name, limits.file_bytes as usize)?;
        let view = open_office(std::io::Cursor::new(&bytes[..]), limits, || Ok(()))?;
        Ok(crate::documents::office_view(&view))
    }
    /// End a PDF session opened from a vault (`None`: from any vault).
    pub(crate) fn forget_vault_pdf(&self, handle: Option<&str>) {
        if let Ok(mut slot) = self.pdf.lock() {
            if slot.as_ref().is_some_and(|(k, _)| matches!(k, PdfKey::Vault { handle: h, .. } if handle.is_none_or(|x| x == h))) { slot.take(); }
        }
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// A page of a PDF inside a 1.8 vault, rendered by the isolated worker.
    pub fn vault_pdf_page(&self, handle: String, path: Vec<String>, name: String, page: u32, width: u32) -> Result<NativePdfPage> {
        self.with_vault_pdf(VaultKind::Paranoic, &handle, &path, &name, |s| Ok(NativePdfPage { pages: s.pages(), index: page, png: s.render(page, width)? }))
    }
    pub fn vault_pdf_search(&self, handle: String, path: Vec<String>, name: String, query: String, start: u32) -> Result<NativePdfMatches> {
        self.with_vault_pdf(VaultKind::Paranoic, &handle, &path, &name, |s| { let (pages, next_page) = s.search(&query, start)?; Ok(NativePdfMatches { pages, next_page }) })
    }
    /// Read-only view of a DOCX/XLSX/ODS inside a 1.8 vault (parsed in memory).
    pub fn vault_office_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeOfficeView> {
        self.vault_office(VaultKind::Paranoic, &handle, &path, &name)
    }
    /// As `vault_pdf_page`, for a Cryptomator vault.
    pub fn cryptomator_pdf_page(&self, handle: String, path: Vec<String>, name: String, page: u32, width: u32) -> Result<NativePdfPage> {
        self.with_vault_pdf(VaultKind::Cryptomator, &handle, &path, &name, |s| Ok(NativePdfPage { pages: s.pages(), index: page, png: s.render(page, width)? }))
    }
    pub fn cryptomator_pdf_search(&self, handle: String, path: Vec<String>, name: String, query: String, start: u32) -> Result<NativePdfMatches> {
        self.with_vault_pdf(VaultKind::Cryptomator, &handle, &path, &name, |s| { let (pages, next_page) = s.search(&query, start)?; Ok(NativePdfMatches { pages, next_page }) })
    }
    pub fn cryptomator_office_open(&self, handle: String, path: Vec<String>, name: String) -> Result<NativeOfficeView> {
        self.vault_office(VaultKind::Cryptomator, &handle, &path, &name)
    }
}
