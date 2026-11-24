//! Bridge calls for the integration seams (batch 1): the Rust cores are
//! done; these expose them to the host. Contracts: docs/architecture/CONTRACTS.md.
use crate::*;
use features::{finish, notes_error, run_wait, vault_error};
use paranoic_core::{EntryKind, ResourcePath};
use paranoic_documents::{TextPager, TextPagerLimits};
use paranoic_formats::paranoic_vault::Spool;
use zeroize::Zeroizing;

const PAGERS: usize = 8;
static TEXT_PAGERS: Mutex<Vec<(String, Location, Arc<TextPager>)>> = Mutex::new(Vec::new());

/// E2.6: a large text opened read-only. `rows`: total rows (a row ends at a
/// line break or after 16 KiB of text); `encoding`: as `text_open`.
#[derive(Clone, uniffi::Record)]
pub struct NativeTextPager { pub handle: String, pub rows: u64, pub encoding: u8, pub bytes: u64 }
/// E5.3 export: files written and attachments that stayed inside (R-3).
#[derive(Clone, uniffi::Record)]
pub struct NativeNotesExport { pub files: u64, pub skipped_attachments: u64 }
/// E5.3 import: what was added and files that were not .md/.txt UTF-8 text.
#[derive(Clone, uniffi::Record)]
pub struct NativeNotesImport { pub tabs: u64, pub notes: u64, pub skipped: u64 }
/// E6.1: `strength` 0 weak, 1 fair, 2 strong; `hints`: 1 too short, 2 common
/// password, 3 repeats, 4 sequence, 5 one kind of character, 6 add a word.
#[derive(Clone, uniffi::Record)]
pub struct NativePasswordFeedback { pub strength: u8, pub hints: Vec<u8> }

fn read_limited(core: &Core, location: &Location, limit: u64) -> Result<Vec<u8>> {
    let mut input = core.open_read(location)?;
    let meta = input.metadata()?;
    if meta.kind != EntryKind::File { return Err(BridgeError::Unsupported); }
    if meta.bytes > limit { return Err(BridgeError::Limit); }
    let mut data = Vec::new();
    std::io::Read::by_ref(&mut input).take(limit + 1).read_to_end(&mut data).map_err(|_| BridgeError::Unavailable)?;
    if data.len() as u64 > limit { return Err(BridgeError::Limit); }
    if data.len() as u64 != meta.bytes || input.metadata()? != meta { return Err(BridgeError::Changed); }
    Ok(data)
}

#[uniffi::export]
impl NativeRuntime {
    // ------------------------------------------------------------ E2.6 large text

    /// Index a text file (one pass, nothing kept in memory but the index).
    /// At most 8 open at once; the oldest is closed when a ninth opens.
    pub fn text_pager_open(&self, location: NativeLocation) -> Result<NativeTextPager> {
        let location = location.core()?;
        let pager = TextPager::index(self.core.open_read(&location)?, TextPagerLimits::default(), || Ok(()))?;
        let handle = Uuid::new_v4().to_string();
        let info = NativeTextPager { handle: handle.clone(), rows: pager.rows(), bytes: pager.source().bytes,
            encoding: match pager.encoding() { paranoic_documents::TextEncoding::Utf8 => 0, paranoic_documents::TextEncoding::Utf8Bom => 1,
                paranoic_documents::TextEncoding::Utf16Le => 2, paranoic_documents::TextEncoding::Utf16Be => 3 } };
        let mut pagers = TEXT_PAGERS.lock().map_err(lock_error)?;
        if pagers.len() >= PAGERS { pagers.remove(0); }
        pagers.push((handle, location, Arc::new(pager)));
        Ok(info)
    }
    /// Rows `first .. first+count` (≤ 2000) without line breaks; Changed if the
    /// file is no longer the version that was opened.
    pub fn text_pager_read(&self, handle: String, first: u64, count: u32) -> Result<Vec<String>> {
        let (location, pager) = {
            let pagers = TEXT_PAGERS.lock().map_err(lock_error)?;
            let (_, l, p) = pagers.iter().find(|(h, _, _)| *h == handle).ok_or(BridgeError::Invalid)?;
            (l.clone(), Arc::clone(p))
        };
        let rows = pager.read(self.core.open_read(&location)?, first, count as usize, || Ok(()))?;
        Ok(rows.into_iter().map(|r| r.to_string()).collect())
    }
    pub fn text_pager_close(&self, handle: String) {
        if let Ok(mut pagers) = TEXT_PAGERS.lock() { pagers.retain(|(h, _, _)| *h != handle); }
    }

    // ------------------------------------------------------- E5.3 notes as files

    /// Write every tab as a folder of `.md` notes into a NEW folder `name`
    /// in `parent` (never merged). Plain files (R-5): the UI warns first.
    pub fn notes_export(&self, parent: NativeLocation, name: String) -> Result<NativeNotesExport> {
        let export = self.with_notes(|store, session| Ok(paranoic_notes::export_plain(&store.read_notebook(session).map_err(notes_error)?)))?;
        let parent = parent.core()?;
        let root = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let mut made = std::collections::BTreeSet::new();
        let mut create = |path: ResourcePath| -> Result<()> {
            if !made.insert(path.as_str().to_owned()) { return Ok(()); }
            let job = self.core.submit(Operation::CreateDirectory { target: Location { storage: root.storage, path } })?;
            finish(run_wait(&self.core, job.id)?, 1)
        };
        create(root.path.clone())?;
        for file in &export.files {
            let target = Location { storage: root.storage, path: ResourcePath::parse(format!("{}/{}", root.path.as_str(), file.path))? };
            create(target.path.parent_and_name()?.0)?;
            let spool = Self::spool_path()?;
            let written = (|| -> Result<()> {
                let mut staged = self.app.stage(&spool, paranoic_core::JobId(Uuid::new_v4()))?;
                if staged.write_all(&file.bytes).is_err() { let _ = staged.abort(); return Err(BridgeError::Unavailable); }
                staged.publish()?;
                let job = self.core.submit(Operation::CopyFile { source: Location { storage: self.app.id(), path: spool.clone() },
                    target, conflict: paranoic_core::ConflictPolicy::Fail })?;
                finish(run_wait(&self.core, job.id)?, 1)
            })();
            self.drop_spool(&spool);
            written?;
        }
        Ok(NativeNotesExport { files: export.files.len() as u64, skipped_attachments: export.skipped_attachments as u64 })
    }
    /// Import `.md` / `.txt` from files and folders (a folder's files form a
    /// tab named after it; its subfolders' files tabs of their own). New tabs
    /// are added; with `into_tab` every imported note goes into that tab.
    pub fn notes_import(&self, sources: Vec<NativeLocation>, root_tab: String, into_tab: Option<String>) -> Result<NativeNotesImport> {
        let limits = paranoic_notes::NotesLimits::default();
        let budget = limits.notebook_bytes as u64;
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut total = 0u64;
        let mut take = |prefix: &str, location: &Location, files: &mut Vec<(String, Vec<u8>)>| -> Result<()> {
            let name = location.path.parent_and_name()?.1.to_owned();
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".md") || lower.ends_with(".txt")) { files.push((name, Vec::new())); return Ok(()); }
            let data = read_limited(&self.core, location, budget.saturating_sub(total))?;
            total += data.len() as u64;
            files.push((if prefix.is_empty() { name } else { format!("{prefix}/{name}") }, data));
            Ok(())
        };
        for source in sources {
            let location = source.core()?;
            match self.core.metadata(&location)?.kind {
                EntryKind::File => take("", &location, &mut files)?,
                EntryKind::Directory => {
                    let folder = location.path.parent_and_name()?.1.to_owned();
                    for entry in self.core.list(&location)? {
                        match entry.metadata.kind {
                            EntryKind::File => take(&folder, &entry.location, &mut files)?,
                            EntryKind::Directory => {
                                let sub = entry.location.path.parent_and_name()?.1.to_owned();
                                for inner in self.core.list(&entry.location)? {
                                    if inner.metadata.kind == EntryKind::File { take(&sub, &inner.location, &mut files)?; }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => return Err(BridgeError::Unsupported),
            }
        }
        let imported = paranoic_notes::import_plain(&files, &root_tab, &limits, features::now_ms()).map_err(notes_error)?;
        let notes = imported.tabs.iter().map(|t| t.notes.len()).sum::<usize>() as u64;
        let tabs = imported.tabs.len() as u64;
        self.with_notes(|store, session| {
            let mut book = store.read_notebook(session).map_err(notes_error)?;
            match &into_tab {
                Some(id) => {
                    let tab = book.tabs.iter_mut().find(|t| t.id == *id).ok_or(BridgeError::Invalid)?;
                    for t in &imported.tabs { tab.notes.extend(t.notes.iter().cloned()); }
                }
                None => book.tabs.extend(imported.tabs.iter().cloned()),
            }
            store.save_notebook(session, &book).map_err(notes_error)
        })?;
        Ok(NativeNotesImport { tabs: if into_tab.is_some() { 0 } else { tabs }, notes, skipped: imported.skipped as u64 })
    }

    // ------------------------------------------------------ E5.4 recovery key

    /// Indices (0-based, ascending) of 2 or 3 random groups of the shown key
    /// that the user must type before the window may close.
    pub fn vault_recovery_challenge(&self, count: u8) -> Result<Vec<u32>> {
        Ok(paranoic_formats::paranoic_v1::recovery_challenge(count as usize, &mut paranoic_formats::paranoic_v1::OsRandom)
            .map_err(|_| BridgeError::Invalid)?.into_iter().map(|i| i as u32).collect())
    }
    pub fn vault_recovery_check(&self, text: String, indices: Vec<u32>, answers: Vec<String>) -> bool {
        let text = Zeroizing::new(text);
        let indices: Vec<usize> = indices.into_iter().map(|i| i as usize).collect();
        let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
        paranoic_formats::paranoic_v1::recovery_check(&text, &indices, &answers)
    }
    /// Forgotten password (R-8): the vault was opened with the recovery key
    /// (`vault_open_recovery`); the same key sets a new password.
    pub fn vault_reset_password(&self, handle: String, recovery: String, new_password: String) -> Result<()> {
        let (recovery, new) = (Zeroizing::new(recovery), Zeroizing::new(new_password));
        let key = paranoic_formats::paranoic_v1::parse_recovery_text(&recovery).map_err(|_| BridgeError::Invalid)?;
        let vault = self.vault(&handle)?;
        let spool = Self::spool_path()?;
        let result = vault.plan_reset_password(&self.core, &key, new.as_bytes(), Spool { storage: self.app.as_ref(), path: &spool }, &mut || Ok(()))
            .map_err(vault_error).and_then(|plan| self.run(&plan));
        self.drop_spool(&spool);
        result
    }

    // ------------------------------------------------------------- E6.1 passwords

    pub fn password_feedback(&self, password: String) -> NativePasswordFeedback {
        use paranoic_notes::{PasswordHint as H, PasswordStrength as S};
        let password = Zeroizing::new(password);
        let (strength, hints) = paranoic_notes::password_feedback(&password);
        NativePasswordFeedback { strength: match strength { S::Weak => 0, S::Fair => 1, S::Strong => 2 },
            hints: hints.into_iter().map(|h| match h { H::TooShort => 1, H::Common => 2, H::Repeats => 3, H::Sequence => 4, H::OneKind => 5, H::AddWord => 6 }).collect() }
    }
    /// `words` (4..=10) random words from the EFF list, separated by spaces.
    pub fn generate_passphrase(&self, words: u8) -> Result<String> {
        Ok(paranoic_crypto::generate_passphrase(words as usize, ' ').map_err(|_| BridgeError::Invalid)?.to_string())
    }
}
