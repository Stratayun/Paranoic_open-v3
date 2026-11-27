//! Prepare and journal create-new Cryptomator v8 directory publication.
use paranoic_core::{EntryKind, ErrorCode, Location, Operation, PreparedTreeFile, ResourcePath};
use paranoic_crypto::{CryptoError, Result};
use crate::{CryptomatorDirectoryId, CryptomatorReader};
use crate::cryptomator_reader::storage_error;

/// No plaintext name or unwrapped key is retained. The operation includes the
/// public directory UUID required by dir.c9r; it can correlate vault structure.
/// This is a prepared request, not evidence of successful publication.
pub struct CryptomatorPreparedDirectory {
    vault_id: uuid::Uuid,
    root: ResourcePath,
    binding: String,
    data_path: ResourcePath,
    operation: Operation,
}

impl CryptomatorPreparedDirectory {
    pub fn vault_id(&self) -> uuid::Uuid { self.vault_id }
    pub fn entry_path(&self) -> &ResourcePath { &self.operation.target().path }
    /// The actual hashed data directory, even when publication also creates its
    /// missing d/XX bucket as part of the first tree.
    pub fn data_path(&self) -> &ResourcePath { &self.data_path }
}

impl CryptomatorReader {
    /// Prepare an empty directory with a fresh UUID and encrypted dirid backup.
    /// An absent two-character data bucket is published with the data directory;
    /// an existing bucket is never merged or replaced by this operation.
    /// Does no storage writes. Call off the UI thread with host cancellation.
    pub fn prepare_directory<F: FnMut() -> Result<()>>(&self, parent: &[String], name: &str,
        mut checkpoint: F) -> Result<CryptomatorPreparedDirectory> {
        // Leave one level for the new directory within the reader's depth cap.
        if parent.len() >= 64 { return Err(CryptoError::LimitExceeded); }
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok(()) };
        check()?;
        let vault_id = self.session.id()?;
        let caps = self.storage.capabilities();
        if !caps.write { return Err(CryptoError::Permissions); }
        // Windows permits this operation without a directory sync.
        if !caps.atomic_move_no_replace || !(caps.durable_directory_sync || caps.directory_sync_waived) { return Err(CryptoError::Unsupported); }
        let binding = self.storage.binding_key().map_err(storage_error)?;
        let (parent_id, parent_path) = self.resolve_directory(parent, &mut check)?;
        let entry = self.session.encrypt_name(&parent_id, name)?;
        let entry_path = parent_path.join(entry.component()).map_err(|_| CryptoError::InvalidInput)?;
        require_absent(self, &entry_path)?;
        check()?;
        let id = CryptomatorDirectoryId::generate()?;
        let data_path = self.data_path(&id)?;
        let (bucket, component) = data_path.parent_and_name().map_err(|_| CryptoError::InvalidInput)?;
        let backup = self.session.directory_backup(&id)?;
        let (data_target, backup_path) = match self.storage.metadata(&bucket) {
            Ok(metadata) if metadata.kind == EntryKind::Directory => {
                require_absent(self, &data_path)?;
                (data_path.clone(), ResourcePath::parse("dirid.c9r").map_err(|_| CryptoError::InvalidInput)?)
            }
            Ok(_) => return Err(CryptoError::InvalidInput),
            Err(error) if error.code == ErrorCode::NotFound => {
                // If another writer creates this bucket before execution, the
                // exclusive publication fails. Do not silently change the plan.
                let relative = ResourcePath::root().join(component)
                    .and_then(|path| path.join("dirid.c9r")).map_err(|_| CryptoError::InvalidInput)?;
                (bucket, relative)
            }
            Err(error) => return Err(storage_error(error)),
        };
        let mut files = vec![PreparedTreeFile {
            path: ResourcePath::parse("dir.c9r").map_err(|_| CryptoError::InvalidInput)?,
            bytes: id.as_bytes().to_vec(),
        }];
        if let Some(mapping) = entry.name_mapping() {
            files.push(PreparedTreeFile {
                path: ResourcePath::parse("name.c9s").map_err(|_| CryptoError::InvalidInput)?,
                bytes: mapping.as_bytes().to_vec(),
            });
        }
        let operation = Operation::PublishLinkedTrees {
            data: Location { storage: self.storage.id(), path: data_target },
            data_files: vec![PreparedTreeFile { path: backup_path, bytes: backup }],
            target: Location { storage: self.storage.id(), path: entry_path }, files,
        };
        operation.validate().map_err(|_| CryptoError::InvalidInput)?;
        check()?;
        if self.storage.binding_key().map_err(storage_error)? != binding { return Err(CryptoError::Permissions); }
        check()?;
        Ok(CryptomatorPreparedDirectory { vault_id, root: self.root.clone(),
            binding: blake3::hash(binding.as_bytes()).to_hex().to_string(), data_path, operation })
    }

    /// Persist job_id before admission; retry the identical prepared request
    /// with that ID after an uncertain acknowledgement. Once admitted, the job
    /// contains only format objects and no longer needs an unlocked session.
    /// Host lifecycle policy must cancel admitted jobs through Core explicitly.
    pub fn submit_directory(&self, core: &paranoic_core::Core, prepared: &CryptomatorPreparedDirectory,
        job_id: paranoic_core::JobId) -> paranoic_core::Result<paranoic_core::JobRecord> {
        use paranoic_core::CoreError;
        let vault_id = self.session.id().map_err(|_| CoreError::new(ErrorCode::PermissionDenied, "vault session locked"))?;
        if vault_id != prepared.vault_id || self.root != prepared.root
            || self.storage.id() != prepared.operation.target().storage {
            return Err(CoreError::new(ErrorCode::InvalidInput, "prepared directory belongs to another vault root"));
        }
        let binding = self.storage.binding_key()?;
        if blake3::hash(binding.as_bytes()).to_hex().as_str() != prepared.binding
            || core.storage_binding_fingerprint(self.storage.id())? != prepared.binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "vault storage binding changed"));
        }
        self.session.id().map_err(|_| CoreError::new(ErrorCode::PermissionDenied, "vault session locked"))?;
        core.submit_with_id(job_id, prepared.operation.clone())
    }
}

fn require_absent(reader: &CryptomatorReader, path: &ResourcePath) -> Result<()> {
    match reader.storage.metadata(path) {
        Ok(_) => Err(CryptoError::AlreadyExists),
        Err(error) if error.code == ErrorCode::NotFound => Ok(()),
        Err(error) => Err(storage_error(error)),
    }
}
