//! Cryptomator renames that change the name-shortening class, on SAF
//! (mapped wrapper renames, directory reshapes). Same model as the local
//! adapter: the wrapper is moved into a hidden staging folder
//! ".paranoic-<job>" as "entry", its mapping is changed there (the former
//! mapping is kept as evidence, never deleted before success), then it is
//! renamed and moved into place without replacing. Inspection and recovery
//! work on that staging. SAF has no directory sync (owner decision E2.8).
#![allow(clippy::too_many_arguments)] // mirrors the Storage trait signatures
use crate::android::{exists, status, SafStorage};
use paranoic_core::*;

fn integrity(what: &str) -> CoreError { CoreError::new(ErrorCode::Integrity, what.to_owned()) }
fn join(parent: &str, name: &str) -> String { if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") } }

impl SafStorage {
    fn raw_metadata(&self, path: &str) -> Result<Option<(EntryKind, u64)>> {
        let info = self.host.metadata(self.tree.clone(), path.to_owned());
        if info.error == 5 { return Ok(None); }
        status(info.error)?;
        Ok(Some((match info.kind { 1 => EntryKind::File, 2 => EntryKind::Directory, _ => EntryKind::Other }, info.bytes)))
    }
    fn names(&self, path: &str) -> Result<Vec<String>> {
        let listing = self.host.list(self.tree.clone(), path.to_owned(), 16);
        status(listing.error)?;
        Ok(listing.entries.into_iter().map(|e| e.name).collect())
    }
    fn read_small(&self, path: &str, limit: u64) -> Result<Option<Vec<u8>>> {
        match self.raw_metadata(path)? {
            Some((EntryKind::File, bytes)) if bytes <= limit => {}
            Some(_) => return Ok(None),
            None => return Ok(None),
        }
        let opened = self.host.open(self.tree.clone(), path.to_owned());
        status(opened.error)?;
        let mut data = Vec::new();
        let result = (|| -> Result<()> {
            loop {
                let got = self.host.read(opened.id, data.len() as u64, 4096);
                status(got.error)?;
                if got.data.is_empty() { break; }
                data.extend_from_slice(&got.data);
                if data.len() as u64 > limit { return Err(integrity("mapping too large")); }
            }
            Ok(())
        })();
        self.host.close(opened.id);
        result.map(|_| Some(data))
    }
    fn document(&self, path: &str) -> Result<String> {
        let resolved = self.host.resolve(self.tree.clone(), path.to_owned());
        status(resolved.error)?;
        Ok(resolved.document)
    }
    /// A new file `parent/name` with `bytes` (the name must be free).
    fn write_new(&self, parent: &str, name: &str, bytes: &[u8]) -> Result<()> {
        let created = self.host.create(self.tree.clone(), parent.to_owned(), name.to_owned(), false);
        status(created.error)?;
        if created.name != name {
            let _ = self.host.delete(self.tree.clone(), created.document, false);
            return Err(exists());
        }
        let opened = self.host.open_write(self.tree.clone(), created.document);
        status(opened.error)?;
        if let Err(error) = status(self.host.write(opened.id, bytes.to_vec())) { let _ = self.host.finish_write(opened.id); return Err(error); }
        status(self.host.finish_write(opened.id))
    }
    /// Move `from_parent/name` into `to_parent`, then rename to `new_name`;
    /// each step checked (no replacement anywhere).
    fn relocate(&self, from_parent: &str, name: &str, to_parent: &str, new_name: &str) -> Result<()> {
        let mut current_parent = from_parent.to_owned();
        if from_parent != to_parent {
            if self.raw_metadata(&join(to_parent, name))?.is_some() { return Err(exists()); }
            let moved = self.host.move_document(self.tree.clone(), self.document(&join(from_parent, name))?,
                self.document(from_parent)?, self.document(to_parent)?);
            status(moved.error)?;
            if moved.name != name { return Err(CoreError::new(ErrorCode::NeedsAttention, "moved wrapper got a provider-chosen name")); }
            current_parent = to_parent.to_owned();
        }
        if name != new_name {
            if self.raw_metadata(&join(&current_parent, new_name))?.is_some() { return Err(exists()); }
            let renamed = self.host.rename(self.tree.clone(), self.document(&join(&current_parent, name))?, new_name.to_owned());
            status(renamed.error)?;
            if renamed.name != new_name { return Err(CoreError::new(ErrorCode::NeedsAttention, "wrapper got a provider-chosen name")); }
        }
        Ok(())
    }
    fn staging(&self, parent: &str, job: JobId) -> String { join(parent, &format!(".paranoic-{}", job.0)) }
    fn make_staging(&self, parent: &str, job: JobId) -> Result<String> {
        let name = format!(".paranoic-{}", job.0);
        let created = self.host.create(self.tree.clone(), parent.to_owned(), name.clone(), true);
        status(created.error)?;
        if created.name != name { return Err(CoreError::new(ErrorCode::NeedsAttention, "staging name taken")); }
        Ok(join(parent, &name))
    }
    /// The staged wrapper: "entry", or already renamed to its final name.
    fn staged_name(&self, staging: &str, final_name: &str) -> Result<Option<String>> {
        for name in ["entry", final_name] {
            if matches!(self.raw_metadata(&join(staging, name))?, Some((EntryKind::Directory, _))) { return Ok(Some(name.to_owned())); }
        }
        Ok(None)
    }

    // ------------------------------------------------- mapped (file/dir) renames

    fn observe_mapping(&self, path: &str, old: &[u8], new: &[u8]) -> Result<MappedEntryObservation> {
        use MappedEntryObservation as O;
        match self.raw_metadata(path)? {
            None => return Ok(O::Missing),
            Some((EntryKind::Directory, _)) => {}
            Some(_) => return Ok(O::Different),
        }
        let Some(bytes) = self.read_small(&join(path, "name.c9s"), 368)? else { return Ok(O::Different) };
        Ok(match (bytes == old, bytes == new) {
            (true, true) => O::BothMappings, (true, false) => O::OldMapping, (false, true) => O::NewMapping, _ => O::Different,
        })
    }
    /// Replace `wrapper/name.c9s` with `bytes`: the current one is moved into
    /// `staging` as `evidence` first (kept), then the new one is written.
    fn swap_mapping(&self, staging: &str, wrapper: &str, evidence: &str, bytes: Option<&[u8]>) -> Result<()> {
        let current = join(wrapper, "name.c9s");
        if self.raw_metadata(&current)?.is_some() {
            self.relocate(wrapper, "name.c9s", staging, evidence)?;
        }
        if let Some(bytes) = bytes { self.write_new(wrapper, "name.c9s", bytes)?; }
        Ok(())
    }
    pub(crate) fn saf_rename_mapped(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8]) -> Result<()> {
        if source == target || target.as_str().starts_with(&format!("{}/", source.as_str()))
            || old.is_empty() || new.is_empty() || old.len() > 368 || new.len() > 368 {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped rename"));
        }
        let ((parent, source_name), (target_parent, target_name)) = (source.parent_and_name()?, target.parent_and_name()?);
        if self.raw_metadata(target.as_str())?.is_some() { return Err(exists()); }
        if self.observe_mapping(source.as_str(), old, new)? != MappedEntryObservation::OldMapping {
            return Err(integrity("mapping differs from request"));
        }
        let staging = self.make_staging(parent.as_str(), job)?;
        // From here on the staging owns the only copy: never clean it on error.
        self.relocate(parent.as_str(), source_name, &staging, "entry")?;
        let entry = join(&staging, "entry");
        self.swap_mapping(&staging, &entry, "old-mapping", Some(new))?;
        if self.observe_mapping(&entry, old, new)? != MappedEntryObservation::NewMapping { return Err(integrity("staged mapping not written")); }
        self.relocate(&staging, "entry", &staging, target_name)?;
        self.relocate(&staging, target_name, target_parent.as_str(), target_name)?;
        if self.observe_mapping(target.as_str(), old, new)? != MappedEntryObservation::NewMapping { return Err(integrity("published mapping differs")); }
        // Success: the evidence is no longer needed; the empty staging goes.
        let _ = self.host.delete(self.tree.clone(), self.document(&join(&staging, "old-mapping"))?, false);
        if self.names(&staging)?.is_empty() { let _ = self.host.delete(self.tree.clone(), self.document(&staging)?, true); }
        Ok(())
    }
    pub(crate) fn saf_inspect_mapped(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8],
        check: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
        check()?;
        let (parent, _) = source.parent_and_name()?;
        let (_, target_name) = target.parent_and_name()?;
        let source_state = self.observe_mapping(source.as_str(), old, new)?;
        let target_state = self.observe_mapping(target.as_str(), old, new)?;
        let staging = self.staging(parent.as_str(), job);
        let present = matches!(self.raw_metadata(&staging)?, Some((EntryKind::Directory, _)));
        let staged = match (present, self.staged_name(&staging, target_name)?) {
            (true, Some(name)) => self.observe_mapping(&join(&staging, &name), old, new)?,
            _ => MappedEntryObservation::Missing,
        };
        check()?;
        Ok(MappedRenameObservation { source: source_state, target: target_state, staged_entry: staged, staging_present: present })
    }
    pub(crate) fn saf_recover_mapped(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId,
        old: &[u8], new: &[u8], action: MappedRecoveryAction) -> Result<()> {
        use MappedEntryObservation as O;
        if original == recovery || source == target { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped recovery request")); }
        let ((parent, source_name), (target_parent, target_name)) = (source.parent_and_name()?, target.parent_and_name()?);
        for public in [source.as_str(), target.as_str()] {
            if self.raw_metadata(public)?.is_some() { return Err(CoreError::new(ErrorCode::AlreadyExists, "public rename entry exists; recovery refused")); }
        }
        let staging = self.staging(parent.as_str(), original);
        let staged = self.staged_name(&staging, target_name)?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "rename staging is missing"))?;
        let wrapper = join(&staging, &staged);
        let state = self.observe_mapping(&wrapper, old, new)?;
        if !matches!(state, O::OldMapping | O::NewMapping | O::BothMappings) { return Err(integrity("staged entry has no recognized mapping")); }
        let (expected, destination, name) = match action {
            MappedRecoveryAction::Complete => (new, target_parent.as_str(), target_name),
            MappedRecoveryAction::RestoreSource => (old, parent.as_str(), source_name),
        };
        let wanted = if expected == new { O::NewMapping } else { O::OldMapping };
        if state != wanted && state != O::BothMappings {
            self.swap_mapping(&staging, &wrapper, &format!("old-mapping-{}", recovery.0), Some(expected))?;
        }
        self.relocate(&staging, &staged, &staging, name)?;
        self.relocate(&staging, name, destination, name)?;
        let published = join(destination, name);
        if !matches!(self.observe_mapping(&published, old, new)?, s if s == wanted || s == O::BothMappings) {
            return Err(integrity("recovered mapping differs"));
        }
        Ok(())
    }

    // ------------------------------------------------------ directory reshape

    fn reshape_state(&self, path: &str, layout: &DirectoryReshape) -> Result<MappedEntryObservation> {
        use MappedEntryObservation as O;
        match self.raw_metadata(path)? {
            None => return Ok(O::Missing),
            Some((EntryKind::Directory, _)) => {}
            Some(_) => return Ok(O::Different),
        }
        let names = self.names(path)?;
        let link = self.read_small(&join(path, "dir.c9r"), 64)?;
        if link.as_deref() != Some(layout.link.as_slice()) { return Ok(O::Different); }
        let mapping = if names.iter().any(|n| n == "name.c9s") { self.read_small(&join(path, "name.c9s"), 368)? } else { None };
        let fits = |m: Option<&Vec<u8>>| match m {
            None => names.len() == 1 && mapping.is_none(),
            Some(bytes) => names.len() == 2 && mapping.as_ref() == Some(bytes),
        };
        Ok(match (fits(layout.old_mapping.as_ref()), fits(layout.new_mapping.as_ref())) {
            (true, true) => O::BothMappings, (true, false) => O::OldMapping, (false, true) => O::NewMapping, _ => O::Different,
        })
    }
    pub(crate) fn saf_reshape(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &DirectoryReshape) -> Result<()> {
        layout.validate()?;
        if source == target || target.as_str().starts_with(&format!("{}/", source.as_str())) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape destination"));
        }
        let ((parent, source_name), (target_parent, target_name)) = (source.parent_and_name()?, target.parent_and_name()?);
        if self.raw_metadata(target.as_str())?.is_some() { return Err(exists()); }
        if self.reshape_state(source.as_str(), layout)? != MappedEntryObservation::OldMapping { return Err(integrity("directory wrapper differs from request")); }
        let staging = self.make_staging(parent.as_str(), job)?;
        self.relocate(parent.as_str(), source_name, &staging, "entry")?;
        let entry = join(&staging, "entry");
        self.swap_mapping(&staging, &entry, "old-mapping", layout.new_mapping.as_deref())?;
        if self.reshape_state(&entry, layout)? != MappedEntryObservation::NewMapping { return Err(integrity("staged wrapper differs")); }
        self.relocate(&staging, "entry", &staging, target_name)?;
        self.relocate(&staging, target_name, target_parent.as_str(), target_name)?;
        if self.reshape_state(target.as_str(), layout)? != MappedEntryObservation::NewMapping { return Err(integrity("published wrapper differs")); }
        if let Ok(document) = self.document(&join(&staging, "old-mapping")) { let _ = self.host.delete(self.tree.clone(), document, false); }
        if self.names(&staging)?.is_empty() { let _ = self.host.delete(self.tree.clone(), self.document(&staging)?, true); }
        Ok(())
    }
    pub(crate) fn saf_inspect_reshape(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &DirectoryReshape,
        check: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
        layout.validate()?;
        check()?;
        let (parent, _) = source.parent_and_name()?;
        let (_, target_name) = target.parent_and_name()?;
        let source_state = self.reshape_state(source.as_str(), layout)?;
        let target_state = self.reshape_state(target.as_str(), layout)?;
        let staging = self.staging(parent.as_str(), job);
        let present = matches!(self.raw_metadata(&staging)?, Some((EntryKind::Directory, _)));
        let staged = match (present, self.staged_name(&staging, target_name)?) {
            (true, Some(name)) => self.reshape_state(&join(&staging, &name), layout)?,
            _ => MappedEntryObservation::Missing,
        };
        check()?;
        Ok(MappedRenameObservation { source: source_state, target: target_state, staged_entry: staged, staging_present: present })
    }
    pub(crate) fn saf_recover_reshape(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId,
        layout: &DirectoryReshape, action: MappedRecoveryAction) -> Result<()> {
        use MappedEntryObservation as O;
        layout.validate()?;
        if original == recovery || source == target { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape recovery request")); }
        let ((parent, source_name), (target_parent, target_name)) = (source.parent_and_name()?, target.parent_and_name()?);
        for public in [source.as_str(), target.as_str()] {
            if self.raw_metadata(public)?.is_some() { return Err(CoreError::new(ErrorCode::AlreadyExists, "public reshape entry exists; recovery refused")); }
        }
        let staging = self.staging(parent.as_str(), original);
        let staged = self.staged_name(&staging, target_name)?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "reshape staging is missing"))?;
        let wrapper = join(&staging, &staged);
        let state = self.reshape_state(&wrapper, layout)?;
        let (desired, wanted, destination, name) = match action {
            MappedRecoveryAction::Complete => (layout.new_mapping.as_deref(), O::NewMapping, target_parent.as_str(), target_name),
            MappedRecoveryAction::RestoreSource => (layout.old_mapping.as_deref(), O::OldMapping, parent.as_str(), source_name),
        };
        match state {
            s if s == wanted => {}
            O::OldMapping | O::NewMapping => self.swap_mapping(&staging, &wrapper, &format!("old-mapping-{}", recovery.0), desired)?,
            _ => return Err(integrity("staged wrapper has no recognized layout")),
        }
        if self.reshape_state(&wrapper, layout)? != wanted { return Err(integrity("staged wrapper differs")); }
        self.relocate(&staging, &staged, &staging, name)?;
        self.relocate(&staging, name, destination, name)?;
        if self.reshape_state(&join(destination, name), layout)? != wanted { return Err(integrity("recovered wrapper differs")); }
        Ok(())
    }
}
