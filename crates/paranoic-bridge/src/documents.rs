//! Plan 2.2 and 2.3: text documents (TXT/Markdown — view, edit, save) and
//! Office documents (DOCX, XLSX, ODS — structured read-only view).
//! Saving never overwrites a change made elsewhere: the file is replaced by one
//! journaled Core job only if it still has the version that was opened.
use crate::*;
use features::{finish, run_wait, StageSink};
use paranoic_documents::{open_office_location, Block, OfficeLimits, OfficeView, TextDocument, TextEncoding};

/// Text files larger than this are not opened for editing.
pub(crate) const TEXT_LIMIT: usize = 16 << 20;

/// Version of the file when it was opened; pass back unchanged to `text_save`.
#[derive(Clone, uniffi::Record)]
pub struct NativeFileVersion {
    pub bytes: u64, pub modified_ms: Option<u64>,
    /// Authoritative version of encoded contents; mtime is only informational.
    pub blake3: String,
}

/// `encoding`: 1 UTF-8, 2 UTF-8 with BOM, 3 UTF-16 LE, 4 UTF-16 BE (kept on save).
#[derive(Clone, uniffi::Record)]
pub struct NativeTextDocument { pub text: String, pub encoding: u8, pub version: NativeFileVersion }

#[derive(Clone, uniffi::Record)]
pub struct NativeRun { pub text: String, pub bold: bool, pub italic: bool, pub underline: bool }

/// `kind` 1 paragraph (`heading`: 1–9 heading, 0 title, null body; `list`), 2 table (`rows`).
#[derive(Clone, uniffi::Record)]
pub struct NativeOfficeBlock { pub kind: u8, pub heading: Option<u8>, pub list: bool, pub runs: Vec<NativeRun>, pub rows: Vec<Vec<String>> }

#[derive(Clone, uniffi::Record)]
pub struct NativeSheet { pub name: String, pub rows: Vec<Vec<String>>, pub truncated: bool }

/// `kind` 1 document (DOCX: `blocks`), 2 workbook (XLSX/ODS: `sheets`).
/// `truncated`: the view hit a size limit and is incomplete.
#[derive(Clone, uniffi::Record)]
pub struct NativeOfficeView { pub kind: u8, pub blocks: Vec<NativeOfficeBlock>, pub sheets: Vec<NativeSheet>, pub truncated: bool }

pub(crate) fn encoding_code(e: TextEncoding) -> u8 { match e { TextEncoding::Utf8 => 1, TextEncoding::Utf8Bom => 2, TextEncoding::Utf16Le => 3, TextEncoding::Utf16Be => 4 } }

impl NativeRuntime {
    fn publish_text(&self, document: &TextDocument, submit: impl FnOnce(&paranoic_documents::PreparedArtifact, Location)
        -> paranoic_core::Result<paranoic_core::JobRecord>) -> Result<NativeFileVersion> {
        let mut spool = spool::PublicationSpool::new(self)?;
        let staged = self.app.stage(spool.path(), JobId(Uuid::new_v4()))?;
        document.prepare_save(Box::new(StageSink(Some(staged))), || Ok(())).map_err(BridgeError::from).and_then(|artifact| {
            let job = submit(&artifact, spool.handoff())?;
            finish(run_wait(&self.core, job.id)?, 1)?;
            spool.completed();
            // Never adopt metadata/content from a later writer as our version.
            Ok(NativeFileVersion { bytes: artifact.bytes(), modified_ms: None,
                blake3: blake3::Hash::from(*artifact.blake3()).to_hex().to_string() })
        })
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Open a text file (UTF-8 / UTF-16 with or without BOM, ≤ 16 MiB).
    pub fn text_open(&self, location: NativeLocation) -> Result<NativeTextDocument> {
        let document = TextDocument::open_location(&self.core, &location.core()?, TEXT_LIMIT, || Ok(()))?;
        let opened = document.opened_metadata().ok_or(BridgeError::Unavailable)?;
        Ok(NativeTextDocument { text: document.text().to_owned(), encoding: encoding_code(document.encoding()),
            version: NativeFileVersion { bytes: opened.bytes, modified_ms: opened.modified_unix_ms,
                blake3: blake3::Hash::from(*document.original_blake3()).to_hex().to_string() } })
    }
    /// Save `text` over the file, keeping its encoding — only if the file still
    /// has `version` (else `Changed`, nothing written: the user decides, e.g.
    /// "save as"). Returns the new version. Blocking.
    pub fn text_save(&self, location: NativeLocation, version: NativeFileVersion, text: String) -> Result<NativeFileVersion> {
        let target = location.core()?;
        let mut document = TextDocument::open_location(&self.core, &target, TEXT_LIMIT, || Ok(()))?;
        let opened = document.opened_metadata().cloned().ok_or(BridgeError::Unavailable)?;
        if opened.bytes != version.bytes || blake3::Hash::from(*document.original_blake3()).to_hex().as_str() != version.blake3 {
            return Err(BridgeError::Changed);
        }
        let expected_hash = *document.original_blake3();
        let length = document.text().len();
        document.replace(document.revision(), 0..length, &text)?;
        self.publish_text(&document, |artifact, spool|
            artifact.submit_replace_content(&self.core, spool, target.clone(), opened.clone(), expected_hash, JobId(Uuid::new_v4())))
    }
    /// Save `text` (UTF-8) as a NEW file `target_parent/name` (never replaces). Blocking.
    pub fn text_save_as(&self, target_parent: NativeLocation, name: String, text: String) -> Result<NativeFileVersion> {
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let mut document = TextDocument::open(&b""[..], TEXT_LIMIT, || Ok(()))?;
        document.replace(document.revision(), 0..0, &text)?;
        self.publish_text(&document, |artifact, spool| artifact.submit_new(&self.core, spool, target.clone(), JobId(Uuid::new_v4())))
    }
    /// Read-only structured view of a DOCX, XLSX or ODS file (no macros, no
    /// external content; bounded). Blocking.
    pub fn office_open(&self, location: NativeLocation) -> Result<NativeOfficeView> {
        let limits = if cfg!(target_os = "android") { OfficeLimits::phone() } else { OfficeLimits::default() };
        let view = open_office_location(&self.core, &location.core()?, limits, || Ok(()))?;
        Ok(office_view(&view))
    }
}

/// Bridge form of a parsed Office document.
pub(crate) fn office_view(view: &OfficeView) -> NativeOfficeView {
        match view {
            OfficeView::Word(word) => NativeOfficeView { kind: 1, sheets: vec![], truncated: word.truncated,
                blocks: word.blocks.iter().map(|b| match b {
                    Block::Paragraph { heading, list, runs, .. } => NativeOfficeBlock { kind: 1, heading: *heading, list: *list, rows: vec![],
                        runs: runs.iter().map(|r| NativeRun { text: r.text.clone(), bold: r.bold, italic: r.italic, underline: r.underline }).collect() },
                    Block::Table { rows } => NativeOfficeBlock { kind: 2, heading: None, list: false, runs: vec![], rows: rows.clone() },
                }).collect() },
            OfficeView::Workbook(book) => NativeOfficeView { kind: 2, blocks: vec![], truncated: book.truncated,
                sheets: book.sheets.iter().map(|s| NativeSheet { name: s.name.clone(), rows: s.rows.clone(), truncated: s.truncated }).collect() },
        }
}
