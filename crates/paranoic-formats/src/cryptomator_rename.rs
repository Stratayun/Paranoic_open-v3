//! Namespace renames and moves within one shortening class through Core.
use paranoic_core::{CoreError, EntryKind, ErrorCode, Location, Metadata, Operation, ResourcePath};
use paranoic_crypto::{CryptoError, Result};
use crate::{CryptomatorListedKind, CryptomatorReader};
use crate::cryptomator_reader::storage_error;

/// Contains ciphertext paths and descriptive metadata, never plaintext names.
/// The host must serialize namespace writers from preparation through job end.
/// Metadata checks do not pin an immutable source version.
pub struct CryptomatorPreparedRename {
    vault_id: uuid::Uuid,
    root: ResourcePath,
    binding: String,
    source: Location,
    target: Location,
    metadata: Metadata,
    mappings: Option<(Vec<u8>, Vec<u8>)>,
    reshape: Option<paranoic_core::DirectoryReshape>,
}

impl CryptomatorPreparedRename {
    pub fn source_path(&self) -> &ResourcePath { &self.source.path }
    pub fn target_path(&self) -> &ResourcePath { &self.target.path }
}

impl CryptomatorReader {
    /// Rename within one plaintext parent, without changing file contents or
    /// directory UUIDs. Directories can change shortening class; file transitions remain unsupported.
    pub fn prepare_rename<F: FnMut() -> Result<()>>(&self, parent: &[String],
        old_name: &str, new_name: &str, checkpoint: F) -> Result<CryptomatorPreparedRename> {
        self.prepare_move(parent, old_name, parent, new_name, checkpoint)
    }

    /// Move within this vault; encrypt the new name under its destination parent.
    /// Directory UUIDs and content ciphertext stay unchanged. Namespace writers
    /// must be serialized until the submitted job finishes. Mixed shortening
    /// classes are supported for directory wrappers here; files use
    /// `prepare_file_reshape`. Directory moves to a deeper parent walk the
    /// subtree (bounded) to keep every path within 64 components. Submit with `submit_rename`.
    pub fn prepare_move<F: FnMut() -> Result<()>>(&self, parent: &[String],
        old_name: &str, target_parent: &[String], new_name: &str,
        mut checkpoint: F) -> Result<CryptomatorPreparedRename> {
        if parent.len() >= 64 || target_parent.len() >= 64
            || (parent == target_parent && old_name == new_name) { return Err(CryptoError::InvalidInput); }
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok(()) };
        check()?;
        let vault_id = self.session.id()?;
        let caps = self.storage.capabilities();
        if !caps.write || !caps.delete { return Err(CryptoError::Permissions); }
        // Windows permits this operation without a directory sync.
        if !caps.atomic_move_no_replace || !(caps.durable_directory_sync || caps.directory_sync_waived) { return Err(CryptoError::Unsupported); }
        let binding = self.storage.binding_key().map_err(storage_error)?;
        let (directory, path) = self.resolve_directory(parent, &mut check)?;
        let old = self.session.encrypt_name(&directory, old_name)?;
        let (target_directory, target_path) = self.resolve_directory(target_parent, &mut check)?;
        let new = self.session.encrypt_name(&target_directory, new_name)?;
        let mappings = match (old.name_mapping(), new.name_mapping()) {
            (None, None) => None,
            (Some(old), Some(new)) => Some((old.as_bytes().to_vec(), new.as_bytes().to_vec())),
            _ => None,
        };
        let mixed = old.name_mapping().is_some() != new.name_mapping().is_some();
        let mut reshape = None;
        let source = Location { storage: self.storage.id(),
            path: path.join(old.component()).map_err(|_| CryptoError::InvalidInput)? };
        let target = Location { storage: self.storage.id(),
            path: target_path.join(new.component()).map_err(|_| CryptoError::InvalidInput)? };
        if source == target { return Err(CryptoError::InvalidInput); }
        let metadata = self.storage.metadata(&source.path).map_err(storage_error)?;
        if let Some(expected) = old.name_mapping() {
            let mapping_path = source.path.join("name.c9s").map_err(|_| CryptoError::InvalidInput)?;
            let actual = crate::cryptomator_reader::read_small(self.storage.as_ref(), &mapping_path, 368, &mut check)?;
            if actual.as_slice() != expected.as_bytes() { return Err(CryptoError::Authentication); }
        }
        match metadata.kind {
            EntryKind::File if old.name_mapping().is_none() && !mixed => {},
            EntryKind::Directory => {
                let (kind, _) = self.wrapper_kind(&source.path, old.name_mapping().is_some(), &mut check)?;
                if kind == CryptomatorListedKind::UnsupportedLink || (mixed && kind != CryptomatorListedKind::Directory) {
                    return Err(CryptoError::Unsupported);
                }
                if kind == CryptomatorListedKind::Directory {
                // Resolve the link and require an existing confined data folder.
                let mut components = parent.to_vec();
                components.push(old_name.to_owned());
                let (_, moved_data) = self.resolve_directory(&components, &mut check)?;
                let (_, checked_target) = self.resolve_directory_excluding(target_parent, Some(&moved_data), &mut check)?;
                if checked_target != target_path { return Err(CryptoError::RevisionConflict); }
                // A subtree readable before the move must stay within the
                // reader's 64-component limit afterwards: measure its depth.
                if target_parent.len() > parent.len() {
                    let tree = self.subtree_directories(&moved_data, crate::CryptomatorTreeLimits::default(), &mut check)?;
                    let depth = tree.iter().map(|(_, depth)| *depth).max().unwrap_or(0);
                    if target_parent.len() + 1 + depth > 64 { return Err(CryptoError::LimitExceeded); }
                }
                if mixed {
                    let link_path = source.path.join("dir.c9r").map_err(|_| CryptoError::InvalidInput)?;
                    let link = crate::cryptomator_reader::read_small(self.storage.as_ref(), &link_path, 36, &mut check)?;
                    let layout = paranoic_core::DirectoryReshape { link: link.to_vec(),
                        old_mapping: old.name_mapping().map(|v| v.as_bytes().to_vec()),
                        new_mapping: new.name_mapping().map(|v| v.as_bytes().to_vec()) };
                    layout.validate().map_err(|_| CryptoError::InvalidInput)?;
                    reshape = Some(layout);
                }
                }
            }
            _ => return Err(CryptoError::Unsupported),
        }
        match self.storage.metadata(&target.path) {
            Ok(_) => return Err(CryptoError::AlreadyExists),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(storage_error(error)),
        }
        if self.storage.metadata(&source.path).map_err(storage_error)? != metadata
            || self.storage.binding_key().map_err(storage_error)? != binding { return Err(CryptoError::RevisionConflict); }
        check()?;
        Ok(CryptomatorPreparedRename { vault_id, root: self.root.clone(),
            binding: blake3::hash(binding.as_bytes()).to_hex().to_string(), source, target, metadata, mappings, reshape })
    }

    /// Persist job_id first. An existing identical job is returned before source
    /// probing, since its original path may already have been renamed. Once
    /// admitted, cancellation and uncertain completion follow Core::Move rules.
    pub fn submit_rename(&self, core: &paranoic_core::Core, prepared: &CryptomatorPreparedRename,
        job_id: paranoic_core::JobId) -> paranoic_core::Result<paranoic_core::JobRecord> {
        let vault_id = self.session.id().map_err(session_error)?;
        if vault_id != prepared.vault_id || self.root != prepared.root || self.storage.id() != prepared.source.storage {
            return Err(CoreError::new(ErrorCode::InvalidInput, "rename belongs to another vault root"));
        }
        let binding = self.storage.binding_key()?;
        if blake3::hash(binding.as_bytes()).to_hex().as_str() != prepared.binding
            || core.storage_binding_fingerprint(self.storage.id())? != prepared.binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "rename storage binding changed"));
        }
        let operation = if let Some(layout) = &prepared.reshape {
            Operation::ReshapeDirectory { source: prepared.source.clone(), target: prepared.target.clone(), layout: layout.clone() }
        } else { match &prepared.mappings {
            Some((old, new)) => Operation::RenameMapped { source: prepared.source.clone(), target: prepared.target.clone(),
                old_mapping: old.clone(), new_mapping: new.clone() },
            None => Operation::Move { source: prepared.source.clone(), target: prepared.target.clone() },
        }};
        match core.job(job_id) {
            Ok(job) if job.operation == operation => return Ok(job),
            Ok(_) => return Err(CoreError::new(ErrorCode::InvalidInput, "rename job ID already used")),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(error),
        }
        let caps = core.capabilities(self.storage.id())?;
        if !caps.atomic_move_no_replace || !(caps.durable_directory_sync || caps.directory_sync_waived) {
            return Err(CoreError::new(ErrorCode::Unsupported, "durable rename unavailable"));
        }
        if self.storage.metadata(&prepared.source.path)? != prepared.metadata {
            return Err(CoreError::new(ErrorCode::SourceChanged, "rename source changed before admission"));
        }
        self.session.id().map_err(session_error)?;
        core.submit_with_id(job_id, operation)
    }
}

fn session_error(error: CryptoError) -> CoreError {
    CoreError::new(if matches!(error, CryptoError::Busy) { ErrorCode::Busy } else { ErrorCode::PermissionDenied },
        "vault session unavailable")
}
