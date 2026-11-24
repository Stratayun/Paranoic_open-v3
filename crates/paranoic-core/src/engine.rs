use std::{io::Read, path::Path, sync::{Arc, Condvar, Mutex}, thread::{self, JoinHandle}};

use uuid::Uuid;

use crate::{journal::Journal, Capabilities, ConflictPolicy, CoreConfig, CoreError, DirectoryEntry, EntryKind,
    ErrorCode, JobId, JobPage, JobRecord, JobState, Location, Metadata, Operation, OperationResult, Outcome,
    ReadHandle, Result, StagedWrite, StorageId, StorageRegistry};

struct Gate {
    stopping: bool,
    fault: Option<CoreError>,
}

struct Shared {
    journal: Journal,
    storage: StorageRegistry,
    config: CoreConfig,
    gate: Mutex<Gate>,
    wake: Condvar,
}

/// One durable mutation worker. Poll job snapshots from the UI or a host adapter;
/// perform synchronous reads and close() off the UI thread.
pub struct Core {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Core {
    /// Unsealed journal (records are plaintext JSON). Prefer `open_sealed`.
    pub fn open(state_directory: impl AsRef<Path>, config: CoreConfig, storage: StorageRegistry) -> Result<Self> {
        Self::open_sealed(state_directory, config, storage, None)
    }

    /// Journal records (paths, names) are stored only through `sealer`
    /// (e.g. an Android Keystore / DPAPI-bound key). Existing plaintext
    /// records are sealed and the database rebuilt; a sealed journal cannot be
    /// opened without its sealer.
    pub fn open_sealed(state_directory: impl AsRef<Path>, config: CoreConfig, storage: StorageRegistry,
        sealer: Option<Arc<dyn crate::JournalSealer>>) -> Result<Self> {
        config.validate()?;
        let journal = Journal::open(state_directory.as_ref(), sealer, config.journal_retention_days)?;
        journal.bind_storage(&storage.bindings()?)?;
        journal.recover()?;
        let shared = Arc::new(Shared {
            journal, storage, config,
            gate: Mutex::new(Gate { stopping: false, fault: None }),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new().name("paranoic-files".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker_loop(&worker_shared)));
            let fault = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some(CoreError::new(ErrorCode::Internal, "operation worker terminated unexpectedly")),
            };
            if let Some(error) = fault {
                if let Ok(mut gate) = worker_shared.gate.lock() {
                    gate.fault = Some(error);
                    gate.stopping = true;
                    worker_shared.wake.notify_all();
                }
            }
        }).map_err(CoreError::io)?;
        Ok(Self { shared, worker: Some(worker) })
    }

    /// Add a newly consented grant without restarting the worker. The binding is
    /// durable before exposure, serialized with submit, and never replaces an ID.
    pub fn register_storage(&self, provider: Arc<dyn crate::Storage>) -> Result<()> {
        if provider.domain() != crate::StorageDomain::Files {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "private storage cannot enter the file registry"));
        }
        // Storage I/O (binding_key may query the provider) happens before the
        // admission gate; only the journal binding and registry insert are serialized.
        let key = provider.binding_key()?;
        if key.is_empty() { return Err(CoreError::new(ErrorCode::InvalidInput, "empty storage binding")); }
        let digest = blake3::hash(key.as_bytes()).to_hex().to_string();
        let legacy: Vec<String> = provider.legacy_binding_keys()?.iter().filter(|k| !k.is_empty() && **k != key)
            .map(|k| blake3::hash(k.as_bytes()).to_hex().to_string()).collect();
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)?;
        match self.shared.storage.get(provider.id()) {
            Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "storage ID already registered")),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(error),
        }
        // Not registered, so no job of this ID can be running; an identity
        // format upgrade of the same root is applied once (e.g. WebDAV v2).
        self.shared.journal.bind_storage_upgrading(provider.id(), &digest, &legacy)?;
        self.shared.storage.register_shared(provider)
    }

    /// Withdraw a grant from the registry (user revoked it). Refused while a
    /// queued or running job refers to it; serialized with submit, so no new
    /// job can slip in between the check and the removal. Jobs that need
    /// attention stay in the journal and resume once the same grant returns.
    pub fn unregister_storage(&self, id: crate::StorageId) -> Result<()> {
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)?;
        for state in [JobState::Queued, JobState::Running] {
            let mut after = 0;
            loop {
                let page = self.shared.journal.page(after, Some(state), 500)?;
                if page.jobs.iter().any(|job| job.operation.locations().iter().any(|l| l.storage == id)) {
                    return Err(CoreError::new(ErrorCode::Busy, "an unfinished job still uses this storage"));
                }
                match page.next_cursor { Some(next) => after = next, None => break }
            }
        }
        self.shared.storage.unregister(id)
    }

    pub fn submit(&self, operation: Operation) -> Result<JobRecord> {
        self.submit_with_id(JobId(Uuid::new_v4()), operation)
    }

    /// Reuse this ID only for the exact same request after an uncertain submit
    /// acknowledgement. It returns the existing job; it does not repeat it.
    pub fn submit_with_id(&self, id: JobId, operation: Operation) -> Result<JobRecord> {
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)?;
        match self.shared.journal.get(id) {
            Ok(record) if record.operation == operation => return Ok(record),
            Ok(_) => return Err(CoreError::new(ErrorCode::InvalidInput, "job ID was already used for a different request")),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(error),
        }
        preflight(&self.shared, &operation)?;
        let record = self.shared.journal.enqueue(id, operation, self.shared.config.max_queued_jobs)?;
        self.shared.wake.notify_one();
        Ok(record)
    }

    pub fn job(&self, id: JobId) -> Result<JobRecord> { self.shared.journal.get(id) }
    pub fn recent_jobs(&self, limit: usize) -> Result<Vec<JobRecord>> { self.shared.journal.recent(limit) }
    pub fn cancel(&self, id: JobId) -> Result<JobRecord> { self.shared.journal.cancel(id) }

    /// Use after=0 for the first page. Filtering NeedsAttention makes recovery
    /// discoverable even when unresolved jobs are older than the recent list.
    /// Pages are individual snapshots, not one transaction across UI requests.
    pub fn jobs(&self, after: u64, state: Option<JobState>, limit: usize) -> Result<JobPage> {
        self.shared.journal.page(after, state, limit)
    }

    /// NeedsAttention jobs without an explicit publication acceptance. Accepted
    /// jobs remain visible in jobs()/job() with their original state and error.
    /// This is an acknowledgement inbox, not proof that staging was cleaned.
    pub fn unacknowledged_attention(&self, after: u64, limit: usize) -> Result<JobPage> {
        self.shared.journal.unacknowledged_attention(after, limit)
    }

    /// The registered storage behind `id` (for format adapters that work on a
    /// storage directly, e.g. Cryptomator). Mutations still go through jobs.
    pub fn storage(&self, id: StorageId) -> Result<std::sync::Arc<dyn crate::Storage>> { self.shared.storage.get(id) }
    /// The user reviewed a NeedsAttention job (looked at both locations, decided
    /// by hand) and closes it. Changes nothing on storage; removes the job from
    /// `unacknowledged_attention`. History keeps the original state and error.
    pub fn mark_reviewed(&self, id: JobId) -> Result<JobRecord> {
        self.health()?;
        self.shared.journal.mark_reviewed(id)
    }
    /// Compare a caller-held grant with the immutable provider registered here.
    pub fn storage_binding_fingerprint(&self, storage: StorageId) -> Result<String> {
        let key = self.shared.storage.get(storage)?.binding_key()?;
        Ok(blake3::hash(key.as_bytes()).to_hex().to_string())
    }

    /// Read-only reconciliation evidence for interrupted prepared imports.
    /// Does not change job state or resolve ambiguous publication automatically.
    pub fn inspect_expected_copy(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::ExpectedCopyObservation>
    {
        use crate::ExpectedCopyObservation as Observation;
        let record = self.shared.journal.get(id)?;
        if matches!(record.state, crate::JobState::Queued | crate::JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "job is still active"));
        }
        let (target, bytes, hash, bundle) = match &record.operation {
            Operation::CopyExpected { target, bytes, blake3, .. } => (target, *bytes, *blake3, None),
            Operation::CopyExpectedBundle { target, bytes, blake3, content_name, sidecar_name, sidecar, .. } =>
                (target, *bytes, *blake3, Some((content_name, sidecar_name, sidecar))),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no prepared-content expectation")),
        };
        let check = || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "inspection cancelled"))
            } else { Ok(()) }
        };
        check()?;
        let storage = self.shared.storage.get(target.storage)?;
        let before = match storage.metadata(&target.path) {
            Ok(value) => value,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(Observation::Missing),
            Err(error) => return Err(error),
        };
        let expected_kind = if bundle.is_some() { EntryKind::Directory } else { EntryKind::File };
        if before.kind != expected_kind { return Ok(Observation::Differs); }
        let verify = |path: &crate::ResourcePath, size: u64, digest: [u8; 32]| -> Result<bool> {
            check()?;
            if size > self.shared.config.max_file_bytes {
                return Err(CoreError::new(ErrorCode::LimitExceeded, "inspection size exceeds limits"));
            }
            let mut reader = storage.open_read(path)?;
            let initial = reader.metadata()?;
            if initial.kind != EntryKind::File || initial.bytes != size { return Ok(false); }
            let mut buffer = vec![0u8; self.shared.config.copy_buffer_bytes];
            let mut total = 0u64;
            let mut hasher = blake3::Hasher::new();
            loop {
                check()?;
                let count = match reader.read(&mut buffer) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    result => result.map_err(CoreError::io)?,
                };
                if count == 0 { break; }
                total = total.checked_add(count as u64).ok_or_else(CoreError::internal)?;
                if total > size { return Ok(false); }
                hasher.update(&buffer[..count]);
            }
            Ok(total == size && hasher.finalize() == blake3::Hash::from(digest) && reader.metadata()? == initial)
        };
        let matched = if let Some((content, name, data)) = bundle {
            let rows = storage.list(&target.path, 3)?;
            if rows.len() != 2 { return Ok(Observation::Differs); }
            let content_path = target.path.join(content)?;
            let sidecar_path = target.path.join(name)?;
            if !rows.iter().any(|row| row.location.storage == target.storage && row.location.path == content_path)
                || !rows.iter().any(|row| row.location.storage == target.storage && row.location.path == sidecar_path)
            { return Ok(Observation::Differs); }
            verify(&content_path, bytes, hash)?
                && verify(&sidecar_path, data.len() as u64, *blake3::hash(data).as_bytes())?
        } else { verify(&target.path, bytes, hash)? };
        check()?;
        let unchanged = storage.metadata(&target.path)? == before;
        Ok(if matched && unchanged { Observation::Matches } else { Observation::Differs })
    }

    /// Inspect the persisted request after a failed/interrupted bootstrap.
    /// Only terminal jobs are accepted. No key material is regenerated, no
    /// temporary objects are removed and no job state is changed. The caller's
    /// cancellation flag is independent of the historical job cancellation.
    pub fn inspect_prepared_tree(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::PreparedTreeObservation> {
        let record = self.shared.journal.get(id)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "job is still active"));
        }
        record.operation.validate()?;
        let (target, files) = match &record.operation {
            Operation::PublishTree { target, files } => (target, files),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no prepared tree")),
        };
        let provider = self.shared.storage.get(target.storage)?;
        crate::bootstrap::inspect(provider.as_ref(), target, files, &self.shared.config, &mut |_| {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "bootstrap inspection cancelled"))
            } else { Ok(()) }
        })
    }

    /// Observe both parts of a terminal linked publication without modifying
    /// either tree or its job. Missing data and a visible entry are reported
    /// independently; neither authorizes recreating the missing data.
    pub fn inspect_linked_trees(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::LinkedTreesObservation> {
        let record = self.shared.journal.get(id)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "job is still active"));
        }
        record.operation.validate()?;
        let (data, data_files, target, files) = match &record.operation {
            Operation::PublishLinkedTrees { data, data_files, target, files } => (data, data_files, target, files),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no linked publication")),
        };
        let provider = self.shared.storage.get(target.storage)?;
        let mut check = |_| {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "linked publication inspection cancelled"))
            } else { Ok(()) }
        };
        let data = crate::bootstrap::inspect(provider.as_ref(), data, data_files, &self.shared.config, &mut check)?;
        let entry = crate::bootstrap::inspect(provider.as_ref(), target, files, &self.shared.config, &mut check)?;
        check(0)?;
        Ok(crate::LinkedTreesObservation { data, entry })
    }

    /// Explicit host decision to accept currently matching bootstrap bytes.
    /// Always performs fresh readback before the first durable acceptance; an
    /// earlier inspection result is never accepted as authorization. Repeating
    /// an acknowledged ID returns the original receipt without rereading storage.
    /// To observe current bytes again, call inspect_prepared_tree separately.
    /// No filesystem mutation, job replay or automatic staging cleanup occurs.
    pub fn accept_prepared_tree(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<JobRecord> {
        self.health()?;
        let record = self.shared.journal.get(id)?;
        if record.state != JobState::NeedsAttention || !record.effects_started {
            return Err(CoreError::new(ErrorCode::InvalidInput, "bootstrap job does not need recovery acknowledgement"));
        }
        let (target, files) = match &record.operation {
            Operation::PublishTree { target, files } => (target, files),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no prepared tree")),
        };
        if record.bootstrap_acceptance.is_some() { return Ok(record); }
        let check = || {
            self.health()?;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "bootstrap acknowledgement cancelled"))
            } else { Ok(()) }
        };
        check()?;
        let provider = self.shared.storage.get(target.storage)?;
        let binding = *blake3::hash(provider.binding_key()?.as_bytes()).as_bytes();
        let observation = crate::bootstrap::inspect(provider.as_ref(), target, files,
            &self.shared.config, &mut |_| check())?;
        if observation != crate::PreparedTreeObservation::Matches {
            return Err(CoreError::new(ErrorCode::Integrity, "bootstrap must match before acknowledgement"));
        }
        if *blake3::hash(provider.binding_key()?.as_bytes()).as_bytes() != binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "bootstrap storage binding changed"));
        }
        // No storage I/O while holding the admission gate. Shutdown cannot
        // cross this final journal transition; commit wins a late cancellation.
        check()?;
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)?;
        self.shared.journal.accept_publication(&record, binding, cancelled)
    }

    /// Explicitly accept both currently matching parts of a linked publication.
    /// Missing or changed data/entry is never accepted partially. First acceptance
    /// performs new reads; repeated calls return the same historical receipt.
    /// This acknowledges observed content, not fsync, cleanup or an atomic pair.
    pub fn accept_linked_trees(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<JobRecord> {
        self.health()?;
        let record = self.shared.journal.get(id)?;
        if record.state != JobState::NeedsAttention || !record.effects_started {
            return Err(CoreError::new(ErrorCode::InvalidInput, "linked job does not need recovery acknowledgement"));
        }
        let (data, data_files, target, files) = match &record.operation {
            Operation::PublishLinkedTrees { data, data_files, target, files } => (data, data_files, target, files),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no linked publication")),
        };
        if record.linked_trees_acceptance.is_some() { return Ok(record); }
        let check = || {
            self.health()?;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "linked acknowledgement cancelled"))
            } else { Ok(()) }
        };
        check()?;
        let provider = self.shared.storage.get(target.storage)?;
        let binding = *blake3::hash(provider.binding_key()?.as_bytes()).as_bytes();
        // Read both complete trees, then reobserve data after reading the link.
        // These reads detect ordinary changes but cannot pin external writers.
        for (location, contents) in [(data, data_files), (target, files), (data, data_files)] {
            let observed = crate::bootstrap::inspect(provider.as_ref(), location, contents,
                &self.shared.config, &mut |_| check())?;
            if observed != crate::PreparedTreeObservation::Matches {
                return Err(CoreError::new(ErrorCode::Integrity, "both linked trees must match before acknowledgement"));
            }
        }
        if *blake3::hash(provider.binding_key()?.as_bytes()).as_bytes() != binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "linked publication storage binding changed"));
        }
        check()?;
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)?;
        self.shared.journal.accept_publication(&record, binding, cancelled)
    }

    /// Inspect source, destination and private staging of a terminal rename.
    /// Never changes job state, republishes a staged entry or removes artifacts.
    pub fn inspect_mapped_rename(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::MappedRenameObservation> {
        let record = self.shared.journal.get(id)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "rename job is still active"));
        }
        record.operation.validate()?;
        let (source, target, old, new) = match &record.operation {
            Operation::RenameMapped { source, target, old_mapping, new_mapping } => (source, target, old_mapping, new_mapping),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no mapped rename")),
        };
        self.shared.storage.get(source.storage)?.inspect_mapped_rename(&source.path, &target.path, id, old, new, &mut || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "rename inspection cancelled"))
            } else { Ok(()) }
        })
    }

    /// Explicitly queue completion or restoration of the original retained
    /// payload. Persist recovery_id first; reuse it only for this same decision.
    pub fn recover_mapped_rename(&self, original: JobId, recovery_id: JobId,
        action: crate::MappedRecoveryAction) -> Result<JobRecord> {
        if original == recovery_id {
            return Err(CoreError::new(ErrorCode::InvalidInput, "recovery requires a separate job ID"));
        }
        let record = self.shared.journal.get(original)?;
        let (source, target) = match &record.operation {
            Operation::RenameMapped { source, target, .. } => (source, target),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "original job is not a mapped rename")),
        };
        let target = match action { crate::MappedRecoveryAction::Complete => target,
            crate::MappedRecoveryAction::RestoreSource => source }.clone();
        self.submit_with_id(recovery_id, Operation::RecoverMapped { original, target, action })
    }

    /// Inspect source, destination and private staging of a terminal
    /// ReshapeDirectory job. Read-only; never republishes or cleans up.
    pub fn inspect_directory_reshape(&self, id: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::MappedRenameObservation> {
        let record = self.shared.journal.get(id)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "reshape job is still active"));
        }
        record.operation.validate()?;
        let (source, target, layout) = match &record.operation {
            Operation::ReshapeDirectory { source, target, layout } => (source, target, layout),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "job has no directory reshape")),
        };
        self.shared.storage.get(source.storage)?.inspect_reshape_directory(&source.path, &target.path, id, layout, &mut || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                Err(CoreError::new(ErrorCode::Cancelled, "reshape inspection cancelled"))
            } else { Ok(()) }
        })
    }

    /// Explicitly queue completion or restoration of a retained reshape wrapper.
    /// Persist recovery_id first; reuse it only for this same decision.
    pub fn recover_directory_reshape(&self, original: JobId, recovery_id: JobId,
        action: crate::MappedRecoveryAction) -> Result<JobRecord> {
        if original == recovery_id {
            return Err(CoreError::new(ErrorCode::InvalidInput, "recovery requires a separate job ID"));
        }
        let record = self.shared.journal.get(original)?;
        let (source, target) = match &record.operation {
            Operation::ReshapeDirectory { source, target, .. } => (source, target),
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "original job is not a directory reshape")),
        };
        let target = match action { crate::MappedRecoveryAction::Complete => target,
            crate::MappedRecoveryAction::RestoreSource => source }.clone();
        self.submit_with_id(recovery_id, Operation::RecoverReshape { original, target, action })
    }

    /// Observe the original locations of a terminal reshape recovery job.
    pub fn inspect_reshape_recovery(&self, recovery: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::MappedRenameObservation> {
        let record = self.shared.journal.get(recovery)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "recovery job is still active"));
        }
        match record.operation {
            Operation::RecoverReshape { original, .. } => self.inspect_directory_reshape(original, cancelled),
            _ => Err(CoreError::new(ErrorCode::Unsupported, "job is not a reshape recovery")),
        }
    }

    /// Follow a terminal recovery job back to the original storage locations.
    /// Observes current state even when publication succeeded but its result
    /// acknowledgement was interrupted. Does not repeat the recovery action.
    pub fn inspect_mapped_recovery(&self, recovery: JobId, cancelled: &std::sync::atomic::AtomicBool)
        -> Result<crate::MappedRenameObservation> {
        let record = self.shared.journal.get(recovery)?;
        if matches!(record.state, JobState::Queued | JobState::Running) {
            return Err(CoreError::new(ErrorCode::Busy, "recovery job is still active"));
        }
        match record.operation {
            Operation::RecoverMapped { original, .. } => self.inspect_mapped_rename(original, cancelled),
            _ => Err(CoreError::new(ErrorCode::Unsupported, "job is not a mapped recovery")),
        }
    }

    pub fn capabilities(&self, storage: StorageId) -> Result<Capabilities> {
        self.shared.storage.capabilities(storage)
    }

    /// Documents and preview adapters consume this stream without gaining a
    /// mutation handle or an ambient filesystem path. Host owns its lifetime.
    pub fn open_read(&self, location: &Location) -> Result<Box<dyn ReadHandle>> {
        self.health()?;
        let provider = self.shared.storage.get(location.storage)?;
        require_capability(provider.capabilities().read)?;
        provider.open_read(&location.path)
    }

    pub fn health(&self) -> Result<()> {
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        ensure_live(&gate)
    }

    pub fn metadata(&self, location: &Location) -> Result<Metadata> {
        self.health()?;
        let provider = self.shared.storage.get(location.storage)?;
        require_capability(provider.capabilities().read)?;
        provider.metadata(&location.path)
    }

    pub fn list(&self, location: &Location) -> Result<Vec<DirectoryEntry>> {
        self.health()?;
        let provider = self.shared.storage.get(location.storage)?;
        require_capability(provider.capabilities().read)?;
        provider.list(&location.path, self.shared.config.max_list_entries)
    }

    /// Listing for display: entries whose names cannot be represented as
    /// portable paths (or are ambiguous duplicates) are omitted and counted.
    /// Never use it to plan copies/deletions — those require `list`.
    pub fn list_for_display(&self, location: &Location) -> Result<crate::PartialListing> {
        self.health()?;
        let provider = self.shared.storage.get(location.storage)?;
        require_capability(provider.capabilities().read)?;
        provider.list_for_display(&location.path, self.shared.config.max_list_entries)
    }

    /// Read-only inventory for recursive operations and folder size previews.
    /// Cancellation is cooperative; invoke off the UI thread. The returned
    /// metadata is not an immutable snapshot of externally editable files.
    pub fn scan_tree(&self, location: &Location, limits: crate::TreeLimits, cancelled: &std::sync::atomic::AtomicBool) -> Result<crate::TreeInventory> {
        self.health()?;
        let provider = self.shared.storage.get(location.storage)?;
        crate::tree::scan(provider.as_ref(), location, limits, self.shared.config.max_list_entries, || {
            self.health()?;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(CoreError::new(ErrorCode::Cancelled, "tree traversal cancelled"));
            }
            Ok(())
        })
    }

    /// Stops accepting work. The current stream observes this between chunks;
    /// queued requests stay durable and resume on the next successful open().
    pub fn request_shutdown(&self) -> Result<()> {
        let mut gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        gate.stopping = true;
        self.shared.wake.notify_all();
        Ok(())
    }

    pub fn close(mut self) -> Result<()> {
        self.request_shutdown()?;
        self.join_worker()?;
        let gate = self.shared.gate.lock().map_err(|_| CoreError::internal())?;
        match &gate.fault {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn join_worker(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| CoreError::new(ErrorCode::Internal, "cannot join operation worker"))?;
        }
        Ok(())
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // Do not detach a writer that might still mutate storage after shutdown.
        let _ = self.request_shutdown();
        let _ = self.join_worker();
    }
}

fn ensure_live(gate: &Gate) -> Result<()> {
    if let Some(error) = &gate.fault { return Err(error.clone()); }
    if gate.stopping { return Err(CoreError::new(ErrorCode::Shutdown, "core is shutting down")); }
    Ok(())
}

fn worker_loop(shared: &Shared) -> Result<()> {
    loop {
        let record = {
            let mut gate = shared.gate.lock().map_err(|_| CoreError::internal())?;
            loop {
                if gate.stopping { return Ok(()); }
                if let Some(record) = shared.journal.claim()? { break record; }
                gate = shared.wake.wait(gate).map_err(|_| CoreError::internal())?;
            }
        };
        let result = execute(shared, &record);
        // A failed result commit stops the entire worker. Continuing could hide
        // an applied mutation behind a running/unknown journal record.
        shared.journal.finish(record.id, result)?;
    }
}

fn preflight(shared: &Shared, operation: &Operation) -> Result<()> {
    operation.validate()?;
    if let Operation::PublishTree { target, files } = operation {
        crate::bootstrap::validate(target, files, &shared.config)?;
    }
    if let Operation::PublishLinkedTrees { data, data_files, target, files } = operation {
        crate::bootstrap::validate(data, data_files, &shared.config)?;
        crate::bootstrap::validate(target, files, &shared.config)?;
    }
    if let Operation::RecoverMapped { original, target, action } = operation {
        let record = shared.journal.get(*original)?;
        if record.state != JobState::NeedsAttention || !record.effects_started {
            return Err(CoreError::new(ErrorCode::InvalidInput, "original rename is not recoverable"));
        }
        let expected = match &record.operation {
            Operation::RenameMapped { source, target, .. } => match action {
                crate::MappedRecoveryAction::Complete => target,
                crate::MappedRecoveryAction::RestoreSource => source,
            },
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "original job is not a mapped rename")),
        };
        if target != expected { return Err(CoreError::new(ErrorCode::InvalidInput, "recovery destination differs from original request")); }
    }
    if let Operation::RecoverReshape { original, target, action } = operation {
        let record = shared.journal.get(*original)?;
        if record.state != JobState::NeedsAttention || !record.effects_started {
            return Err(CoreError::new(ErrorCode::InvalidInput, "original reshape is not recoverable"));
        }
        let expected = match &record.operation {
            Operation::ReshapeDirectory { source, target, .. } => match action {
                crate::MappedRecoveryAction::Complete => target,
                crate::MappedRecoveryAction::RestoreSource => source,
            },
            _ => return Err(CoreError::new(ErrorCode::Unsupported, "original job is not a directory reshape")),
        };
        if target != expected { return Err(CoreError::new(ErrorCode::InvalidInput, "recovery destination differs from original request")); }
    }
    let target = shared.storage.get(operation.target().storage)?;
    let capabilities = target.capabilities();
    match operation {
        Operation::DeleteFile { .. } | Operation::DeleteFileExpected { .. } | Operation::DeleteEmptyDirectory { .. } => require_capability(capabilities.delete)?,
        Operation::DeleteFileContent { .. } => {
            require_capability(capabilities.delete && capabilities.read)?;
            if !capabilities.conditional_move_source {
                return Err(CoreError::new(ErrorCode::Unsupported, "storage cannot pin the verified file; not deleted"));
            }
        }
        Operation::DeleteDirectory { .. } => require_capability(capabilities.delete && capabilities.read)?,
        Operation::CopyVerified { .. } | Operation::CopyExpected { .. } => require_capability(capabilities.write && capabilities.read)?,
        Operation::ReplaceExpected { .. } | Operation::ReplaceContentExpected { .. } => { require_capability(capabilities.write && capabilities.read && capabilities.delete)?; require_atomic(capabilities.atomic_move_no_replace)? }
        Operation::PublishTree { .. } | Operation::PublishLinkedTrees { .. } | Operation::CopyExpectedBundle { .. } => {
            require_capability(capabilities.write && capabilities.read)?;
            require_atomic(capabilities.atomic_move_no_replace && (capabilities.durable_directory_sync || capabilities.directory_sync_waived))?;
        }
        Operation::MoveDirectoryAcross { source, .. } | Operation::MoveDirectoryExpected { source, .. } => {
            // Same per-file guarantees as MoveFileAcross; otherwise only copying is offered.
            require_capability(capabilities.write && capabilities.read)?;
            require_atomic(capabilities.durable_directory_sync)?;
            if !capabilities.pinned_destination {
                return Err(CoreError::new(ErrorCode::Unsupported, "destination cannot be protected during source removal; copy instead"));
            }
            let source_caps = shared.storage.get(source.storage)?.capabilities();
            if !source_caps.conditional_move_source {
                return Err(CoreError::new(ErrorCode::Unsupported, "source does not support safe conditional removal; copy instead"));
            }
            require_capability(source_caps.read && source_caps.delete)?;
        }
        Operation::MoveFileAcross { source, .. } => {
            require_capability(capabilities.write && capabilities.read)?;
            require_atomic(capabilities.durable_directory_sync)?;
            if !capabilities.pinned_destination {
                return Err(CoreError::new(ErrorCode::Unsupported, "destination cannot be protected during source removal"));
            }
            let source_caps = shared.storage.get(source.storage)?.capabilities();
            if !source_caps.conditional_move_source {
                return Err(CoreError::new(ErrorCode::Unsupported, "source does not support safe conditional removal"));
            }
            require_capability(source_caps.read && source_caps.delete)?;
        }
        Operation::ReshapeDirectory { .. } | Operation::RecoverReshape { .. } | Operation::RecoverMapped { .. } | Operation::RenameMapped { .. } => {
            require_capability(capabilities.read && capabilities.write && capabilities.delete)?;
            // SAF (E2.8): the wrapper is staged and the former mapping kept as
            // evidence until success, so inspection/recovery work without sync.
            require_atomic(capabilities.atomic_move_no_replace && (capabilities.durable_directory_sync || capabilities.directory_sync_waived))?;
        }
        Operation::Move { .. } => {
            require_capability(capabilities.write && capabilities.delete)?;
            if !capabilities.atomic_move_no_replace {
                return Err(CoreError::new(ErrorCode::Unsupported, "storage cannot move atomically without replacement"));
            }
        }
        _ => require_capability(capabilities.write)?,
    }
    if let Some(source) = operation.source() {
        require_capability(shared.storage.get(source.storage)?.capabilities().read)?;
    }
    Ok(())
}

fn require_capability(enabled: bool) -> Result<()> {
    if enabled { Ok(()) } else { Err(CoreError::new(ErrorCode::PermissionDenied, "storage grant does not permit this operation")) }
}
/// The grant allows writing, but the storage cannot do it safely (e.g. SAF:
/// no atomic rename without replacement, no directory sync). This is not a
/// permission problem and must not be shown as "access denied" (phone, У-64).
fn require_atomic(enabled: bool) -> Result<()> {
    if enabled { Ok(()) } else { Err(CoreError::new(ErrorCode::Unsupported, "storage cannot publish atomically (no rename without replacement or directory sync)")) }
}

fn check_cancel(shared: &Shared, id: JobId) -> Result<()> {
    let stopping = shared.gate.lock().map_err(|_| CoreError::internal())?.stopping;
    if stopping || shared.journal.get(id)?.cancel_requested {
        return Err(CoreError::new(ErrorCode::Cancelled, "operation cancellation requested"));
    }
    Ok(())
}

fn execute(shared: &Shared, record: &JobRecord) -> Result<OperationResult> {
    preflight(shared, &record.operation)?;
    check_cancel(shared, record.id)?;
    if let Operation::ReplaceExpected { source, target, bytes, blake3, expected_target } = &record.operation {
        return copy_file_expected(shared, record.id, source, target, ConflictPolicy::Replace,
            Expect { target: Some(Some(expected_target)), content: Some((*bytes, *blake3)), ..Expect::default() });
    }
    if let Operation::ReplaceContentExpected { source, target, bytes, blake3, expected_target, expected_target_blake3 } = &record.operation {
        return copy_file_expected(shared, record.id, source, target, ConflictPolicy::Replace,
            Expect { target: Some(Some(expected_target)), content: Some((*bytes, *blake3)),
                target_content: Some(*expected_target_blake3), ..Expect::default() });
    }
    if let Operation::CopyFileExpected { source, target, expected_source, expected_target, keep_both,
        expected_source_revision, expected_target_revision } = &record.operation {
        let conflict = match (expected_target, keep_both) { (Some(_), _) => ConflictPolicy::Replace, (None, true) => ConflictPolicy::KeepBoth, (None, false) => ConflictPolicy::Fail };
        let target_expect = if *keep_both && expected_target.is_none() { None } else { Some(expected_target.as_ref()) };
        return copy_file_expected(shared, record.id, source, target, conflict,
            Expect { target: target_expect, source: Some(expected_source), source_revision: expected_source_revision.as_deref(),
                target_revision: expected_target_revision.as_deref(), ..Expect::default() });
    }
    if let Operation::CopyFile { source, target, conflict } = &record.operation {
        return copy_file(shared, record.id, source, target, *conflict);
    }
    if let Operation::CopyDirectory { source, target, conflict } = &record.operation {
        return copy_directory(shared, record.id, source, target, *conflict);
    }
    if let Operation::DeleteDirectory { target } = &record.operation {
        return delete_directory(shared, record.id, target);
    }
    if let Operation::CopyVerified { source, target } = &record.operation {
        return verified_transfer(shared, record.id, source, target, false, None, None);
    }
    if let Operation::CopyExpected { source, target, bytes, blake3 } = &record.operation {
        return verified_transfer(shared, record.id, source, target, false, Some((*bytes, *blake3)), None);
    }
    if let Operation::CopyExpectedBundle { source, target, bytes, blake3, content_name, sidecar_name, sidecar } = &record.operation {
        return verified_transfer(shared, record.id, source, target, false, Some((*bytes, *blake3)), Some((content_name, sidecar_name, sidecar)));
    }
    if let Operation::MoveDirectoryAcross { source, target } = &record.operation {
        return move_directory_across(shared, record.id, source, target, None);
    }
    if let Operation::MoveDirectoryExpected { source, target, files } = &record.operation {
        return move_directory_across(shared, record.id, source, target, Some(files));
    }
    if let Operation::MoveFileAcross { source, target } = &record.operation {
        return verified_transfer(shared, record.id, source, target, true, None, None);
    }
    if let Operation::PublishLinkedTrees { data, data_files, target, files } = &record.operation {
        return publish_linked_trees(shared, record.id, data, data_files, target, files);
    }
    if let Operation::PublishTree { target, files } = &record.operation {
        return publish_tree(shared, record.id, target, files);
    }
    if let Operation::DeleteFileContent { target, expected, expected_blake3 } = &record.operation {
        return delete_file_content(shared, record.id, target, expected, expected_blake3);
    }
    let target = record.operation.target();
    let provider = shared.storage.get(target.storage)?;
    shared.journal.begin_effects(record.id)?;
    // One atomic call each (mkdir, rename-no-replace, unlink/rmdir, or the
    // provider's single conditional request): a refusal with one of these codes
    // means nothing changed, so the job is a plain failure, not NeedsAttention.
    let atomic = match &record.operation {
        Operation::CreateDirectory { .. } => Some(provider.create_directory(&target.path)),
        Operation::Move { source, .. } => Some(provider.move_no_replace(&source.path, &target.path)),
        Operation::DeleteFile { .. } => Some(provider.delete_file(&target.path)),
        Operation::DeleteFileExpected { expected, expected_revision, .. } => Some(match provider.metadata(&target.path) {
            Ok(current) if current == *expected => match expected_revision {
                Some(revision) => provider.delete_file_at_revision(&target.path, revision),
                None => provider.delete_file(&target.path),
            },
            Ok(_) => Err(CoreError::new(ErrorCode::SourceChanged, "file changed since it was planned; not deleted")),
            Err(error) => Err(error),
        }),
        Operation::DeleteEmptyDirectory { .. } => Some(provider.delete_empty_directory(&target.path)),
        _ => None,
    };
    if let Some(result) = atomic {
        if let Err(error) = result {
            if matches!(error.code, ErrorCode::AlreadyExists | ErrorCode::NotFound | ErrorCode::PermissionDenied
                | ErrorCode::InvalidInput | ErrorCode::InvalidPath | ErrorCode::Unsupported | ErrorCode::SourceChanged) {
                shared.journal.effects_unchanged(record.id)?;
            }
            return Err(error);
        }
        return Ok(OperationResult { outcome: Outcome::Applied, bytes: 0, blake3: None, target: None, revision: None });
    }
    match &record.operation {
        Operation::CreateDirectory { .. } => provider.create_directory(&target.path)?,
        Operation::CreateFile { .. } => provider.stage(&target.path, record.id)?.publish()?,
        Operation::Move { source, .. } => provider.move_no_replace(&source.path, &target.path)?,
        Operation::ReshapeDirectory { source, layout, .. } => provider.reshape_directory(&source.path, &target.path, record.id, layout)?,
        Operation::RecoverMapped { original, action, .. } => {
            let original_record = shared.journal.get(*original)?;
            let Operation::RenameMapped { source, target, old_mapping, new_mapping } = &original_record.operation else {
                return Err(CoreError::internal());
            };
            provider.recover_mapped(&source.path, &target.path, *original, record.id, old_mapping, new_mapping, *action)?;
        },
        Operation::RecoverReshape { original, action, .. } => {
            let original_record = shared.journal.get(*original)?;
            let Operation::ReshapeDirectory { source, target, layout } = &original_record.operation else {
                return Err(CoreError::internal());
            };
            provider.recover_reshape_directory(&source.path, &target.path, *original, record.id, layout, *action)?;
        },
        Operation::RenameMapped { source, old_mapping, new_mapping, .. } =>
            provider.rename_mapped(&source.path, &target.path, record.id, old_mapping, new_mapping)?,
        Operation::DeleteFile { .. } => provider.delete_file(&target.path)?,
        Operation::DeleteFileExpected { expected, expected_revision, .. } => {
            if provider.metadata(&target.path)? != *expected {
                return Err(CoreError::new(ErrorCode::SourceChanged, "file changed since it was planned; not deleted"));
            }
            match expected_revision {
                Some(revision) => provider.delete_file_at_revision(&target.path, revision)?,
                None => provider.delete_file(&target.path)?,
            }
        }
        Operation::DeleteEmptyDirectory { .. } => provider.delete_empty_directory(&target.path)?,
        Operation::PublishLinkedTrees { .. } | Operation::PublishTree { .. } | Operation::CopyFile { .. } | Operation::CopyDirectory { .. } | Operation::DeleteDirectory { .. }
        | Operation::CopyExpectedBundle { .. } | Operation::CopyExpected { .. } | Operation::CopyVerified { .. } | Operation::MoveFileAcross { .. }
        | Operation::MoveDirectoryAcross { .. } | Operation::MoveDirectoryExpected { .. } | Operation::ReplaceExpected { .. }
        | Operation::ReplaceContentExpected { .. } | Operation::CopyFileExpected { .. } | Operation::DeleteFileContent { .. } => return Err(CoreError::internal()),
    }
    Ok(OperationResult { outcome: Outcome::Applied, bytes: 0, blake3: None, target: None, revision: None })
}

enum Resolution { Proceed(Location), Skip, Replace(Metadata) }

/// Decide the destination before any effect. KeepBoth picks the first free
/// "stem (N).ext"; Replace is limited to an existing regular file.
fn resolve_destination(storage: &dyn crate::Storage, target: &Location, conflict: ConflictPolicy, source_is_file: bool)
    -> Result<Resolution> {
    let existing = match storage.metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.code == ErrorCode::NotFound => return Ok(Resolution::Proceed(target.clone())),
        Err(error) => return Err(error),
    };
    match conflict {
        ConflictPolicy::Fail => Err(CoreError::new(ErrorCode::AlreadyExists, "destination already exists")),
        ConflictPolicy::Skip => Ok(Resolution::Skip),
        ConflictPolicy::Replace if source_is_file && existing.kind == EntryKind::File => Ok(Resolution::Replace(existing)),
        ConflictPolicy::Replace => Err(CoreError::new(ErrorCode::Unsupported, "only a file can replace a file")),
        ConflictPolicy::KeepBoth => {
            let (parent, name) = target.path.parent_and_name()?;
            let (stem, extension) = match name.rfind('.') {
                Some(index) if index > 0 && source_is_file => (&name[..index], &name[index..]),
                _ => (name, ""),
            };
            for number in 1..=9999_u32 {
                let Ok(path) = parent.join(&format!("{stem} ({number}){extension}")) else { break };
                match storage.metadata(&path) {
                    Err(error) if error.code == ErrorCode::NotFound =>
                        return Ok(Resolution::Proceed(Location { storage: target.storage, path })),
                    Err(error) => return Err(error),
                    Ok(_) => continue,
                }
            }
            Err(CoreError::new(ErrorCode::LimitExceeded, "no free destination name"))
        }
    }
}

/// ".pnc-" rather than ".paranoic-": the latter is reserved for adapter
/// staging and is rejected by ResourcePath, so it cannot name journaled paths.
fn hidden_sibling(path: &crate::ResourcePath, purpose: &str, id: JobId) -> Result<crate::ResourcePath> {
    let (parent, _) = path.parent_and_name()?;
    parent.join(&format!(".pnc-{purpose}-{}", id.0))
}

fn copy_file(shared: &Shared, id: JobId, source: &Location, target: &Location, conflict: ConflictPolicy) -> Result<OperationResult> {
    copy_file_expected(shared, id, source, target, conflict, Expect::default())
}

/// Version expectations checked while the job runs.
/// `target`: None = no check; Some(None) = must be absent; Some(Some(m)) = must be exactly m.
#[derive(Default)]
struct Expect<'a> { target: Option<Option<&'a Metadata>>, source: Option<&'a Metadata>, content: Option<(u64, [u8; 32])>,
    target_content: Option<[u8; 32]>,
    /// Provider revisions planned by the caller (cloud ETag, У-24).
    source_revision: Option<&'a str>, target_revision: Option<&'a str> }
fn same_revision(provider: &dyn crate::Storage, path: &crate::ResourcePath, expected: Option<&str>) -> Result<bool> {
    match expected { None => Ok(true), Some(expected) => Ok(provider.provider_revision(path)?.as_deref() == Some(expected)) }
}

/// `expect`: (target metadata observed by the caller, prepared length, BLAKE3).
fn copy_file_expected(shared: &Shared, id: JobId, source: &Location, target: &Location, conflict: ConflictPolicy,
    expect: Expect<'_>) -> Result<OperationResult> {
    let input_provider = shared.storage.get(source.storage)?;
    let output_provider = shared.storage.get(target.storage)?;
    let resolution = resolve_destination(output_provider.as_ref(), target, conflict, true)?;
    if let Some(observed) = expect.target {
        // The destination must still be the version that was planned/opened.
        match (&resolution, observed) {
            (Resolution::Replace(existing), Some(observed)) if existing == observed => {}
            (Resolution::Proceed(_), None) => {}
            _ => return Err(CoreError::new(ErrorCode::SourceChanged, "destination changed since it was planned; not written")),
        }
    }
    if !same_revision(output_provider.as_ref(), &target.path, expect.target_revision)? {
        return Err(CoreError::new(ErrorCode::SourceChanged, "destination revision changed since it was planned; not written"));
    }
    let (destination, replaced) = match resolution {
        Resolution::Skip => return Ok(OperationResult { outcome: Outcome::Skipped, bytes: 0, blake3: None, target: None, revision: None }),
        Resolution::Proceed(location) => (location, None),
        Resolution::Replace(existing) => {
            let caps = output_provider.capabilities();
            if !caps.atomic_move_no_replace || !caps.delete {
                return Err(CoreError::new(ErrorCode::Unsupported, "storage cannot replace safely"));
            }
            (target.clone(), Some(existing))
        }
    };
    let mut input = input_provider.open_read(&source.path)?;
    let initial = input.metadata()?;
    if expect.source.is_some_and(|expected| *expected != initial)
        || !same_revision(input_provider.as_ref(), &source.path, expect.source_revision)? {
        return Err(CoreError::new(ErrorCode::SourceChanged, "source changed since it was planned; not copied"));
    }
    if initial.kind != EntryKind::File {
        return Err(CoreError::new(ErrorCode::Unsupported, "only regular files can be copied"));
    }
    if initial.bytes > shared.config.max_file_bytes {
        return Err(CoreError::new(ErrorCode::LimitExceeded, "file exceeds configured size limit"));
    }
    let total = work_total(output_provider.as_ref(), initial.bytes)?;
    shared.journal.progress(id, 0, Some(total))?;
    // Persist the chosen name before any write so a restart never picks another.
    shared.journal.entry_progress(id, 0, 1, Some(destination.clone()))?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    let mut output = output_provider.stage(&destination.path, id)?;
    let (copied, digest) = match stream_copy(shared, id, input.as_mut(), output.as_mut(), &initial, 0, total) {
        Ok(value) => value,
        Err(error) => {
            output.abort()?;
            return Err(error);
        }
    };
    if let Some((bytes, blake3)) = expect.content {
        if copied != bytes || digest != blake3::Hash::from(blake3).to_hex().as_str() {
            output.abort()?;
            return Err(CoreError::new(ErrorCode::Integrity, "prepared content changed"));
        }
    }
    // The bytes read must still be the planned revision (a same-size edit
    // during the download would pass the size/time check).
    if !same_revision(input_provider.as_ref(), &source.path, expect.source_revision)? {
        output.abort()?;
        return Err(CoreError::new(ErrorCode::SourceChanged, "source revision changed during copy"));
    }
    if let Some(time) = initial.modified_unix_ms { let _ = output.set_modified(time); }
    let renamed = (destination != *target).then(|| destination.clone());
    let mut applied = OperationResult { outcome: Outcome::Applied, bytes: copied, blake3: Some(digest), target: renamed, revision: None };
    let Some(existing) = replaced else {
        // Cancellation after this point cannot undo a published file. Commit wins.
        return match publish_checked(shared, id, output, Some((copied, total))) {
            Ok(()) => { applied.revision = output_provider.provider_revision(&destination.path).ok().flatten(); Ok(applied) }
            Err(error) if error.code == ErrorCode::AlreadyExists && conflict == ConflictPolicy::Skip => {
                Ok(OperationResult { outcome: Outcome::Skipped, bytes: 0, blake3: None, target: None, revision: None })
            }
            Err(error) => Err(error),
        };
    };
    // Replace: move the old file aside (journaled), publish, then delete it.
    let backup = Location { storage: destination.storage, path: hidden_sibling(&destination.path, "replaced", id)? };
    if let Err(error) = check_cancel(shared, id).and_then(|_| shared.journal.entry_progress(id, 0, 1, Some(backup.clone()))) {
        output.abort()?;
        return Err(error);
    }
    if let Err(error) = output_provider.move_no_replace(&destination.path, &backup.path) {
        output.abort()?;
        return Err(error);
    }
    let unchanged = (|| {
        if output_provider.metadata(&backup.path)? != existing {
            return Err(CoreError::new(ErrorCode::SourceChanged, "destination changed before replacement"));
        }
        if let Some(hash) = expect.target_content {
            let mut previous = output_provider.open_read(&backup.path)?;
            verify_stream(shared, id, previous.as_mut(), existing.bytes, blake3::Hash::from(hash).to_hex().as_str())
                .map_err(|e| if e.code == ErrorCode::Integrity {
                    CoreError::new(ErrorCode::SourceChanged, "destination contents changed before replacement") } else { e })?;
        }
        Ok(())
    })();
    if let Err(error) = unchanged {
        // Restore on read/cancellation errors too; never discard the previous file.
        let aborted = output.abort();
        output_provider.move_no_replace(&backup.path, &destination.path).map_err(|_|
            CoreError::new(ErrorCode::NeedsAttention, "replaced file changed; previous version retained in hidden backup"))?;
        aborted?;
        return Err(error);
    }
    if let Err(error) = publish_checked(shared, id, output, Some((copied, total))) {
        return match output_provider.move_no_replace(&backup.path, &destination.path) {
            Ok(()) => Err(error),
            Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "replacement failed; previous version retained in hidden backup")),
        };
    }
    output_provider.delete_file(&backup.path).map_err(|_|
        CoreError::new(ErrorCode::NeedsAttention, "file replaced; previous version could not be removed from hidden backup"))?;
    shared.journal.entry_progress(id, 1, 1, None)?;
    applied.revision = output_provider.provider_revision(&destination.path).ok().flatten();
    Ok(applied)
}

/// Progress total of a single-file job: content once, twice when the target
/// uploads it again on publication (`Storage::remote_publication`).
fn work_total(target: &dyn crate::Storage, bytes: u64) -> Result<u64> {
    if target.remote_publication() { bytes.checked_mul(2).ok_or_else(CoreError::internal) } else { Ok(bytes) }
}

/// Publish with cancellation checks. For a single file on a remote target
/// (`upload` = Some((bytes, total))) the upload is reported as progress after
/// the staged `bytes` (never backwards).
fn publish_checked(shared: &Shared, id: JobId, output: Box<dyn StagedWrite>, upload: Option<(u64, u64)>) -> Result<()> {
    let mut last = 0_u64;
    output.publish_checked(&mut |sent| {
        check_cancel(shared, id)?;
        if let Some((bytes, total)) = upload {
            if bytes < total && (sent.saturating_sub(last) >= shared.config.progress_interval_bytes || sent >= bytes) {
                shared.journal.progress(id, bytes.saturating_add(sent.min(bytes)).min(total), Some(total))?;
                last = sent;
            }
        }
        Ok(())
    })
}

fn stream_copy(shared: &Shared, id: JobId, input: &mut dyn ReadHandle, output: &mut dyn StagedWrite, initial: &Metadata, offset: u64, total: u64) -> Result<(u64, String)> {
    // File contents pass through this buffer; erase it when the copy ends.
    let mut buffer = zeroize::Zeroizing::new(vec![0_u8; shared.config.copy_buffer_bytes]);
    let mut hash = blake3::Hasher::new();
    let mut copied = 0_u64;
    let mut checkpoint = 0_u64;
    loop {
        check_cancel(shared, id)?;
        let count = match input.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            value => value.map_err(CoreError::io)?,
        };
        if count == 0 { break; }
        copied = copied.checked_add(count as u64).ok_or_else(CoreError::internal)?;
        if copied > initial.bytes || copied > shared.config.max_file_bytes {
            return Err(CoreError::new(ErrorCode::SourceChanged, "source grew during copy"));
        }
        output.write_all(&buffer[..count]).map_err(CoreError::io)?;
        hash.update(&buffer[..count]);
        if copied - checkpoint >= shared.config.progress_interval_bytes {
            shared.journal.progress(id, offset.checked_add(copied).ok_or_else(CoreError::internal)?, Some(total))?;
            checkpoint = copied;
        }
    }
    if copied != initial.bytes || input.metadata()? != *initial {
        return Err(CoreError::new(ErrorCode::SourceChanged, "source changed during copy"));
    }
    output.flush().map_err(CoreError::io)?;
    shared.journal.progress(id, offset.checked_add(copied).ok_or_else(CoreError::internal)?, Some(total))?;
    check_cancel(shared, id)?;
    Ok((copied, hash.finalize().to_hex().to_string()))
}

struct TreeCopy {
    inventory: crate::TreeInventory,
    targets: Vec<crate::ResourcePath>,
    digests: Vec<Option<String>>,
    result: OperationResult,
}

/// Each file is published atomically; the whole directory is deliberately not
/// advertised as a transaction. Interrupted trees remain for explicit recovery.
fn copy_directory(shared: &Shared, id: JobId, source: &Location, target: &Location, conflict: ConflictPolicy) -> Result<OperationResult> {
    copy_tree(shared, id, source, target, conflict).map(|copy| copy.result)
}

fn copy_tree(shared: &Shared, id: JobId, source: &Location, target: &Location, conflict: ConflictPolicy) -> Result<TreeCopy> {
    let input = shared.storage.get(source.storage)?;
    let output = shared.storage.get(target.storage)?;
    let same_binding = input.binding_key()? == output.binding_key()?;
    let overlaps = |path: &crate::ResourcePath| same_binding
        && (source.path == *path || path.as_str().starts_with(&format!("{}/", source.path.as_str())));
    // Only KeepBoth may name the source itself; it is renamed before any write.
    if overlaps(&target.path) && !(conflict == ConflictPolicy::KeepBoth && source.path == target.path) {
        return Err(CoreError::new(ErrorCode::InvalidInput, "copy destination overlaps source"));
    }
    let skipped = || TreeCopy { inventory: crate::TreeInventory { root: source.clone(), entries: Vec::new(), total_bytes: 0, files: 0, directories: 0 }, targets: Vec::new(),
        digests: Vec::new(), result: OperationResult { outcome: Outcome::Skipped, bytes: 0, blake3: None, target: None, revision: None } };
    let destination = match resolve_destination(output.as_ref(), target, conflict, false)? {
        Resolution::Skip => return Ok(skipped()),
        Resolution::Proceed(location) => location,
        Resolution::Replace(_) => return Err(CoreError::new(ErrorCode::Unsupported, "directories are never replaced")),
    };
    if overlaps(&destination.path) {
        return Err(CoreError::new(ErrorCode::InvalidInput, "copy destination overlaps source"));
    }
    let inventory = crate::tree::scan(input.as_ref(), source, crate::TreeLimits::default(), shared.config.max_list_entries, || check_cancel(shared, id))?;
    let prefix = format!("{}/", source.path.as_str());
    // Resolve and validate every target before the first mutation.
    let mut targets = Vec::with_capacity(inventory.entries.len());
    for entry in &inventory.entries {
        if entry.metadata.kind == EntryKind::File && entry.metadata.bytes > shared.config.max_file_bytes {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "tree contains a file exceeding the size limit"));
        }
        let path = if entry.location == *source {
            destination.path.clone()
        } else {
            let suffix = entry.location.path.as_str().strip_prefix(&prefix)
                .ok_or_else(|| CoreError::new(ErrorCode::Integrity, "tree entry is outside source"))?;
            crate::ResourcePath::parse(format!("{}/{suffix}", destination.path.as_str()))?
        };
        targets.push(path);
    }
    shared.journal.progress(id, 0, Some(inventory.total_bytes))?;
    let entry_total = inventory.entries.len() as u64;
    shared.journal.entry_progress(id, 0, entry_total, Some(destination.clone()))?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    // Root creation is exclusive. Never merge into a raced existing directory.
    if let Err(error) = output.create_directory(&destination.path) {
        if error.code == ErrorCode::AlreadyExists && conflict == ConflictPolicy::Skip {
            return Ok(skipped());
        }
        return Err(error);
    }
    let mut digests = vec![None; inventory.entries.len()];
    let result = (|| {
        let mut copied = 0_u64;
        let mut entries_done = 1_u64;
        shared.journal.entry_progress(id, entries_done, entry_total, None)?;
        for (index, (entry, path)) in inventory.entries.iter().zip(&targets).enumerate().skip(1) {
            check_cancel(shared, id)?;
            shared.journal.entry_progress(id, entries_done, entry_total,
                Some(Location { storage: destination.storage, path: path.clone() }))?;
            if input.metadata(&entry.location.path)? != entry.metadata {
                return Err(CoreError::new(ErrorCode::SourceChanged, "tree entry changed before copy"));
            }
            match entry.metadata.kind {
                EntryKind::Directory => output.create_directory(path)?,
                EntryKind::File => {
                    let mut reader = input.open_read(&entry.location.path)?;
                    if reader.metadata()? != entry.metadata {
                        return Err(CoreError::new(ErrorCode::SourceChanged, "opened tree file changed"));
                    }
                    let mut writer = output.stage(path, id)?;
                    let (bytes, digest) = match stream_copy(shared, id, reader.as_mut(), writer.as_mut(), &entry.metadata, copied, inventory.total_bytes) {
                        Ok(value) => value,
                        Err(error) => { writer.abort()?; return Err(error); }
                    };
                    if let Some(time) = entry.metadata.modified_unix_ms { let _ = writer.set_modified(time); }
                    publish_checked(shared, id, writer, None)?;
                    digests[index] = Some(digest);
                    copied = copied.checked_add(bytes).ok_or_else(CoreError::internal)?;
                }
                _ => return Err(CoreError::new(ErrorCode::Unsupported, "unsupported tree entry")),
            }
            entries_done += 1;
            shared.journal.entry_progress(id, entries_done, entry_total, None)?;
        }
        // Directory times last (children creation updates them), deepest first.
        for (entry, path) in inventory.entries.iter().zip(&targets).rev() {
            if let (EntryKind::Directory, Some(time)) = (entry.metadata.kind, entry.metadata.modified_unix_ms) {
                let _ = output.set_directory_modified(path, time);
            }
        }
        // Best-effort source change detection, not a filesystem snapshot.
        for entry in &inventory.entries {
            check_cancel(shared, id)?;
            if input.metadata(&entry.location.path)? != entry.metadata {
                return Err(CoreError::new(ErrorCode::SourceChanged, "source tree changed during copy"));
            }
        }
        let renamed = (destination != *target).then(|| destination.clone());
        Ok(OperationResult { outcome: Outcome::Applied, bytes: copied, blake3: None, target: renamed, revision: None })
    })();
    let result = result.map_err(|error: CoreError| {
        if error.code == ErrorCode::Cancelled {
            // Earlier files and directories may already exist. Do not claim a
            // clean cancellation or recursively delete externally editable data.
            CoreError::new(ErrorCode::NeedsAttention, "directory copy stopped; partial destination retained")
        } else { error }
    })?;
    Ok(TreeCopy { inventory, targets, digests, result })
}

/// Copy the tree, then remove sources with the same guarantees as
/// MoveFileAcross, per file: the source object is leased (identity pinned),
/// its content is re-read through the lease and must equal the copied digest,
/// the destination is pinned and re-verified, and only then the leased object
/// is removed if both are still unchanged. Directories are removed only when
/// empty, deepest first. Providers without leases/pins are refused in
/// preflight (copy instead); no metadata-only comparison authorizes deletion.
fn move_directory_across(shared: &Shared, id: JobId, source: &Location, target: &Location,
    expected: Option<&[crate::ExpectedTreeFile]>) -> Result<OperationResult> {
    let copy = copy_tree(shared, id, source, target, ConflictPolicy::Fail)?;
    if let Some(expected) = expected {
        // The copied tree must be exactly the prepared one before anything is removed.
        let mismatch = || CoreError::new(ErrorCode::NeedsAttention, "copied tree differs from the prepared manifest; both kept");
        let prefix = format!("{}/", source.path.as_str());
        let mut wanted: std::collections::HashMap<&str, &crate::ExpectedTreeFile> = expected.iter().map(|f| (f.path.as_str(), f)).collect();
        for (entry, digest) in copy.inventory.entries.iter().zip(&copy.digests) {
            if entry.metadata.kind != EntryKind::File { continue; }
            let relative = entry.location.path.as_str().strip_prefix(&prefix).ok_or_else(mismatch)?;
            let file = wanted.remove(relative).ok_or_else(mismatch)?;
            let digest = digest.as_deref().ok_or_else(mismatch)?;
            if entry.metadata.bytes != file.bytes || digest != blake3::Hash::from(file.blake3).to_hex().as_str() { return Err(mismatch()); }
        }
        if !wanted.is_empty() { return Err(mismatch()); }
    }
    let input = shared.storage.get(source.storage)?;
    let output = shared.storage.get(target.storage)?;
    let after_copy = |error: CoreError| if matches!(error.code, ErrorCode::Cancelled | ErrorCode::SourceChanged | ErrorCode::Integrity) {
        CoreError::new(ErrorCode::NeedsAttention, "directory move stopped after copy; inspect both trees")
    } else { error };
    (|| {
        let total = copy.inventory.entries.len() as u64;
        // Entry counters were completed by the copy and may only grow; the
        // removal phase journals the entry being removed as active_entry.
        for ((entry, path), digest) in copy.inventory.entries.iter().zip(&copy.targets).zip(&copy.digests).rev() {
            check_cancel(shared, id)?;
            shared.journal.entry_progress(id, total, total, Some(entry.location.clone()))?;
            match entry.metadata.kind {
                EntryKind::File => {
                    let digest = digest.as_ref().ok_or_else(CoreError::internal)?;
                    let mut lease = input.prepare_move_source(&entry.location.path)?;
                    if lease.reader().metadata()? != entry.metadata {
                        return Err(CoreError::new(ErrorCode::SourceChanged, "source file changed before removal"));
                    }
                    // The leased object must hold exactly the bytes that were copied.
                    verify_stream(shared, id, lease.reader(), entry.metadata.bytes, digest)
                        .map_err(|e| if e.code == ErrorCode::Cancelled { e } else {
                            CoreError::new(ErrorCode::SourceChanged, "source content differs from the copy; not removed") })?;
                    let mut pinned = output.pin_destination(path)?;
                    verify_stream(shared, id, pinned.reader(), entry.metadata.bytes, digest)?;
                    check_cancel(shared, id)?;
                    pinned.still_published()?;
                    lease.remove()?;
                    // Keep the destination pin alive across the destructive step.
                    drop(pinned);
                }
                EntryKind::Directory => {
                    if input.metadata(&entry.location.path)?.kind != EntryKind::Directory {
                        return Err(CoreError::new(ErrorCode::SourceChanged, "source entry changed before removal"));
                    }
                    // Fails if anything new appeared inside: never recursive.
                    input.delete_empty_directory(&entry.location.path)?;
                }
                _ => return Err(CoreError::new(ErrorCode::SourceChanged, "source entry changed before removal")),
            }
        }
        shared.journal.entry_progress(id, total, total, None)?;
        Ok(copy.result.clone())
    })().map_err(after_copy)
}
fn delete_directory(shared: &Shared, id: JobId, target: &Location) -> Result<OperationResult> {
    let storage = shared.storage.get(target.storage)?;
    let inventory = crate::tree::scan(storage.as_ref(), target, crate::TreeLimits::default(), shared.config.max_list_entries, || check_cancel(shared, id))?;
    shared.journal.progress(id, 0, Some(inventory.total_bytes))?;
    let entry_total = inventory.entries.len() as u64;
    shared.journal.entry_progress(id, 0, entry_total, None)?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    let result = (|| {
        let mut removed = 0_u64;
        let mut entries_done = 0_u64;
        // Parents precede descendants in the inventory, so reverse order only
        // removes a directory after processing its recorded children.
        for entry in inventory.entries.iter().rev() {
            check_cancel(shared, id)?;
            shared.journal.entry_progress(id, entries_done, entry_total, Some(entry.location.clone()))?;
            let current = storage.metadata(&entry.location.path)?;
            match entry.metadata.kind {
                EntryKind::File => {
                    if current != entry.metadata {
                        return Err(CoreError::new(ErrorCode::SourceChanged, "file changed before deletion"));
                    }
                    storage.delete_file(&entry.location.path)?;
                    removed = removed.checked_add(entry.metadata.bytes).ok_or_else(CoreError::internal)?;
                    shared.journal.progress(id, removed, Some(inventory.total_bytes))?;
                }
                EntryKind::Directory => {
                    // Our own removals change directory mtime and size. Never
                    // recurse into new children: remove-empty fails instead.
                    if current.kind != EntryKind::Directory {
                        return Err(CoreError::new(ErrorCode::SourceChanged, "directory type changed before deletion"));
                    }
                    storage.delete_empty_directory(&entry.location.path)?;
                }
                _ => return Err(CoreError::new(ErrorCode::Unsupported, "unsupported deletion entry")),
            }
            entries_done += 1;
            shared.journal.entry_progress(id, entries_done, entry_total, None)?;
        }
        Ok(OperationResult { outcome: Outcome::Applied, bytes: removed, blake3: None, target: None, revision: None })
    })();
    result.map_err(|error: CoreError| {
        if error.code == ErrorCode::Cancelled {
            CoreError::new(ErrorCode::NeedsAttention, "directory deletion stopped; some entries may already be removed")
        } else { error }
    })
}

fn verified_transfer(shared: &Shared, id: JobId, source: &Location, target: &Location, remove_source: bool, expected: Option<(u64, [u8; 32])>, bundle: Option<(&str, &str, &[u8])>) -> Result<OperationResult> {
    use crate::{TransferPhase, TransferReceipt};
    let source_storage = shared.storage.get(source.storage)?;
    let target_storage = shared.storage.get(target.storage)?;
    match target_storage.metadata(&target.path) {
        Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "transfer destination exists")),
        Err(error) if error.code == ErrorCode::NotFound => {},
        Err(error) => return Err(error),
    }
    let mut source_lease = if remove_source { Some(source_storage.prepare_move_source(&source.path)?) } else { None };
    let mut ordinary_reader = if remove_source { None } else { Some(source_storage.open_read(&source.path)?) };
    let reader: &mut dyn ReadHandle = match source_lease.as_mut() {
        Some(lease) => lease.reader(),
        None => ordinary_reader.as_mut().ok_or_else(CoreError::internal)?.as_mut(),
    };
    let initial = reader.metadata()?;
    if initial.kind != EntryKind::File || initial.bytes > shared.config.max_file_bytes {
        return Err(CoreError::new(ErrorCode::Unsupported, "transfer requires a regular file within configured limits"));
    }
    if expected.is_some_and(|(bytes, _)| bytes != initial.bytes) {
        return Err(CoreError::new(ErrorCode::Integrity, "prepared source size changed"));
    }
    let total = work_total(target_storage.as_ref(), initial.bytes)?;
    shared.journal.progress(id, 0, Some(total))?;
    shared.journal.transfer(id, TransferReceipt { phase: TransferPhase::Copying, bytes: 0, blake3: String::new() })?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    let mut writer = match bundle {
        Some((content, name, data)) => target_storage.stage_bundle(&target.path, id, content, name, data)?,
        None => target_storage.stage(&target.path, id)?,
    };
    let (bytes, digest) = match stream_copy(shared, id, reader, writer.as_mut(), &initial, 0, total) {
        Ok(value) => value,
        Err(error) => { writer.abort()?; return Err(error); }
    };
    if expected.is_some_and(|(size, hash)| size != bytes || blake3::Hash::from(hash).to_hex().as_str() != digest) {
        writer.abort()?;
        return Err(CoreError::new(ErrorCode::Integrity, "prepared source content changed"));
    }
    let record_phase = |phase| shared.journal.transfer(id, TransferReceipt { phase, bytes, blake3: digest.clone() });
    if let Err(error) = check_cancel(shared, id).and_then(|_| record_phase(TransferPhase::Publishing)) {
        writer.abort()?;
        return Err(error);
    }
    publish_checked(shared, id, writer, Some((bytes, total)))?;
    // After publication, even cancellation leaves a destination that must not
    // be removed or treated as a clean rollback by the general job executor.
    let result = (|| {
        record_phase(TransferPhase::Verifying)?;
        let mut destination_lease = if remove_source { Some(target_storage.pin_destination(&target.path)?) } else { None };
        let content_path = match bundle {
            Some((content, _, _)) => target.path.join(content)?,
            None => target.path.clone(),
        };
        let mut ordinary_destination = if remove_source { None } else { Some(target_storage.open_read(&content_path)?) };
        let destination: &mut dyn ReadHandle = match destination_lease.as_mut() {
            Some(lease) => lease.reader(),
            None => ordinary_destination.as_mut().ok_or_else(CoreError::internal)?.as_mut(),
        };
        verify_stream(shared, id, destination, bytes, &digest)?;
        if let Some((_, name, data)) = bundle {
            let mut sidecar = target_storage.open_read(&target.path.join(name)?)?;
            verify_stream(shared, id, sidecar.as_mut(), data.len() as u64, blake3::hash(data).to_hex().as_str())?;
        }
        record_phase(TransferPhase::Verified)?;
        if remove_source {
            check_cancel(shared, id)?;
            record_phase(TransferPhase::RemovingSource)?;
            destination_lease.as_mut().ok_or_else(CoreError::internal)?.still_published()?;
            source_lease.take().ok_or_else(CoreError::internal)?.remove()?;
            record_phase(TransferPhase::SourceRemoved)?;
        }
        // Explicit drop after removal keeps the destination lease alive across
        // the destructive operation and its journal acknowledgement.
        drop(destination_lease);
        Ok(OperationResult { outcome: Outcome::Applied, bytes, blake3: Some(digest.clone()), target: None, revision: None })
    })();
    result.map_err(|error: CoreError| {
        if error.code == ErrorCode::Cancelled {
            CoreError::new(ErrorCode::NeedsAttention, "transfer stopped after publication; inspect both locations")
        } else { error }
    })
}

/// У-69: pin the object, verify metadata and content through the pin, then
/// remove only that object. Every check happens before any effect.
fn delete_file_content(shared: &Shared, id: JobId, target: &Location, expected: &Metadata, expected_blake3: &[u8; 32]) -> Result<OperationResult> {
    let provider = shared.storage.get(target.storage)?;
    let mut lease = provider.prepare_move_source(&target.path)?;
    let changed = || CoreError::new(ErrorCode::SourceChanged, "file changed since it was verified; not deleted");
    if lease.reader().metadata()? != *expected { return Err(changed()); }
    verify_stream(shared, id, lease.reader(), expected.bytes, blake3::Hash::from(*expected_blake3).to_hex().as_str())
        .map_err(|error| if error.code == ErrorCode::Integrity { changed() } else { error })?;
    if lease.reader().metadata()? != *expected { return Err(changed()); }
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    if let Err(error) = lease.remove() {
        if matches!(error.code, ErrorCode::NotFound | ErrorCode::PermissionDenied | ErrorCode::Unsupported | ErrorCode::SourceChanged) {
            shared.journal.effects_unchanged(id)?;
        }
        return Err(error);
    }
    Ok(OperationResult { outcome: Outcome::Applied, bytes: 0, blake3: None, target: None, revision: None })
}

fn verify_stream(shared: &Shared, id: JobId, reader: &mut dyn ReadHandle, expected_bytes: u64, expected_digest: &str) -> Result<()> {
    let initial = reader.metadata()?;
    if initial.kind != EntryKind::File || initial.bytes != expected_bytes {
        return Err(CoreError::new(ErrorCode::Integrity, "destination size does not match transfer"));
    }
    // File contents pass through this buffer; erase it when the copy ends.
    let mut buffer = zeroize::Zeroizing::new(vec![0_u8; shared.config.copy_buffer_bytes]);
    let mut hash = blake3::Hasher::new();
    let mut bytes = 0_u64;
    loop {
        check_cancel(shared, id)?;
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            value => value.map_err(CoreError::io)?,
        };
        if count == 0 { break; }
        bytes = bytes.checked_add(count as u64).ok_or_else(CoreError::internal)?;
        if bytes > expected_bytes {
            return Err(CoreError::new(ErrorCode::Integrity, "destination grew during verification"));
        }
        hash.update(&buffer[..count]);
    }
    if bytes != expected_bytes || hash.finalize().to_hex().as_str() != expected_digest || reader.metadata()? != initial {
        return Err(CoreError::new(ErrorCode::Integrity, "destination verification failed"));
    }
    Ok(())
}

fn publish_tree(shared: &Shared, id: JobId, target: &Location, files: &[crate::PreparedTreeFile]) -> Result<OperationResult> {
    let total = crate::validate_prepared_tree(files)?;
    let provider = shared.storage.get(target.storage)?;
    match provider.metadata(&target.path) {
        Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "bootstrap destination exists")),
        Err(error) if error.code == ErrorCode::NotFound => {},
        Err(error) => return Err(error),
    }
    for file in files {
        if file.bytes.len() as u64 > shared.config.max_file_bytes {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "bootstrap file exceeds configured limit"));
        }
    }
    shared.journal.progress(id, 0, Some(total))?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    provider.publish_tree(&target.path, id, files, &mut || check_cancel(shared, id))?;
    // Publication has happened: cancellation cannot be reported as a clean abort.
    let result = (|| {
        let mut last_progress = 0;
        let observation = crate::bootstrap::inspect(provider.as_ref(), target, files,
            &shared.config, &mut |done| {
                check_cancel(shared, id)?;
                if done != last_progress {
                    shared.journal.progress(id, done, Some(total))?;
                    last_progress = done;
                }
                Ok(())
            })?;
        if observation != crate::PreparedTreeObservation::Matches {
            return Err(CoreError::new(ErrorCode::Integrity, "published bootstrap differs from prepared tree"));
        }
        Ok(OperationResult { outcome: Outcome::Applied, bytes: total, blake3: None, target: None, revision: None })
    })();
    result.map_err(|error: CoreError| if error.code == ErrorCode::Cancelled {
        CoreError::new(ErrorCode::NeedsAttention, "bootstrap published; readback interrupted")
    } else { error })
}

fn publish_linked_trees(shared: &Shared, id: JobId, data: &Location, data_files: &[crate::PreparedTreeFile],
    target: &Location, files: &[crate::PreparedTreeFile]) -> Result<OperationResult> {
    let provider = shared.storage.get(target.storage)?;
    let data_bytes = crate::validate_prepared_tree(data_files)?;
    let total = data_bytes + crate::validate_prepared_tree(files)?;
    // Reject existing entries at both locations before creating any object.
    // Provider publication still arbitrates create-new races atomically.
    for location in [data, target] {
        match provider.metadata(&location.path) {
            Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "linked publication destination exists")),
            Err(error) if error.code == ErrorCode::NotFound => {},
            Err(error) => return Err(error),
        }
    }
    shared.journal.progress(id, 0, Some(total))?;
    shared.journal.entry_progress(id, 0, 2, Some(data.clone()))?;
    check_cancel(shared, id)?;
    shared.journal.begin_effects(id)?;
    provider.publish_tree(&data.path, id, data_files, &mut || check_cancel(shared, id))?;
    // The first tree is public now. All later cancellation is partial completion,
    // even when the second adapter cleaned its own unpublished staging tree.
    let result = (|| {
        let mut done = 0;
        let observation = crate::bootstrap::inspect(provider.as_ref(), data, data_files,
            &shared.config, &mut |bytes| {
                check_cancel(shared, id)?;
                if bytes != done {
                    shared.journal.progress(id, bytes, Some(total))?;
                    done = bytes;
                }
                Ok(())
            })?;
        if observation != crate::PreparedTreeObservation::Matches {
            return Err(CoreError::new(ErrorCode::Integrity, "linked publication data differs"));
        }
        shared.journal.entry_progress(id, 1, 2, Some(target.clone()))?;
        check_cancel(shared, id)?;
        provider.publish_tree(&target.path, id, files, &mut || check_cancel(shared, id))?;
        let observation = crate::bootstrap::inspect(provider.as_ref(), target, files,
            &shared.config, &mut |bytes| {
                check_cancel(shared, id)?;
                let progress = data_bytes + bytes;
                if progress != done {
                    shared.journal.progress(id, progress, Some(total))?;
                    done = progress;
                }
                Ok(())
            })?;
        if observation != crate::PreparedTreeObservation::Matches {
            return Err(CoreError::new(ErrorCode::Integrity, "linked publication entry differs"));
        }
        // Reobserve data after publishing its reference. No snapshot guarantee
        // is inferred; later external edits remain outside this transaction.
        let observation = crate::bootstrap::inspect(provider.as_ref(), data, data_files,
            &shared.config, &mut |_| check_cancel(shared, id))?;
        if observation != crate::PreparedTreeObservation::Matches {
            return Err(CoreError::new(ErrorCode::Integrity, "linked data changed during entry publication"));
        }
        shared.journal.entry_progress(id, 2, 2, None)?;
        Ok(OperationResult { outcome: Outcome::Applied, bytes: total, blake3: None, target: None, revision: None })
    })();
    result.map_err(|error: CoreError| if error.code == ErrorCode::Cancelled {
        CoreError::new(ErrorCode::NeedsAttention, "linked publication stopped after data publication")
    } else { error })
}

