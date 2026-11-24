//! Ordered, journaled multi-step Cryptomator mutations.
//!
//! A plan is a list of ordinary Core operations with deterministic job IDs.
//! `advance_plan` submits step N only after step N-1 is recorded as Succeeded,
//! so a failed or uncertain step stops the plan instead of running later,
//! destructive steps. Plans contain ciphertext paths and encrypted key files,
//! never plaintext names or unwrapped keys; the host persists the serialized
//! plan before the first `advance_plan` and reuses it after restart.
use paranoic_core::{CoreError, EntryKind, ErrorCode, JobId, JobState, Location, Operation,
    PreparedTreeFile, ResourcePath};
use paranoic_crypto::{CryptoError, Result};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
use crate::{CryptomatorDirectoryId, CryptomatorListedKind, CryptomatorReader, CryptomatorVault};
use crate::cryptomator_reader::{read_small, storage_error};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptomatorPlanKind {
    /// Detach the entry under a hidden name, then delete detached ciphertext.
    Delete,
    /// Publish a new masterkey file, keep the old one as a backup.
    PasswordChange,
    /// Remove the old masterkey backup after the new password was verified.
    RemoveMasterkeyBackup,
    /// File rename/move that changes between .c9r and .c9s shortening.
    ReshapeFile,
}

/// Serializable for host persistence. Contains no secrets; the masterkey file
/// in a PasswordChange plan is password-wrapped (scrypt) key material.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptomatorPlan {
    plan_id: uuid::Uuid,
    vault_id: uuid::Uuid,
    root: ResourcePath,
    binding: String,
    kind: CryptomatorPlanKind,
    steps: Vec<Operation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CryptomatorPlanStatus {
    /// Step `step` was just queued (or was already queued identically).
    Submitted { step: usize, job: JobId },
    /// Step is queued or running; call again later.
    Running { step: usize, job: JobId },
    /// The step ended in a non-success state; later steps were not submitted.
    /// Inspect the job (and its recovery APIs) before any manual decision.
    Stopped { step: usize, job: JobId, state: JobState },
    Completed,
}

/// Bounds for subtree walks (deletion and depth checks).
#[derive(Clone, Copy, Debug)]
pub struct CryptomatorTreeLimits {
    pub max_directories: usize,
    pub max_entries_per_directory: usize,
}
impl Default for CryptomatorTreeLimits {
    fn default() -> Self { Self { max_directories: 4096, max_entries_per_directory: 4096 } }
}

impl CryptomatorPlan {
    pub fn plan_id(&self) -> uuid::Uuid { self.plan_id }
    pub fn kind(&self) -> CryptomatorPlanKind { self.kind }
    pub fn steps(&self) -> &[Operation] { &self.steps }
    /// Deterministic per-step job ID; identical across restarts.
    pub fn job_id(&self, step: usize) -> JobId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paranoic/cryptomator-plan/v1");
        hasher.update(self.plan_id.as_bytes());
        hasher.update(&(step as u64).to_be_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest.as_bytes()[..16]);
        JobId(uuid::Builder::from_random_bytes(bytes).into_uuid())
    }
    pub fn root(&self) -> &ResourcePath { &self.root }
    pub fn vault_id(&self) -> uuid::Uuid { self.vault_id }
    /// Finish an interrupted PasswordChange plan without an unlocked vault.
    /// Between its steps 2 and 3 the vault has no `masterkey.cryptomator`, so
    /// it cannot be opened until the plan completes; the host keeps the plan
    /// (`to_json`) in its private state and calls this before opening again.
    /// Same checks as `advance_plan` except the vault session: storage binding
    /// unchanged and every step on that storage. Other kinds are refused.
    pub fn advance_detached(&self, core: &paranoic_core::Core, storage: paranoic_core::StorageId)
        -> paranoic_core::Result<CryptomatorPlanStatus>
    {
        if self.kind != CryptomatorPlanKind::PasswordChange {
            return Err(CoreError::new(ErrorCode::InvalidInput, "only a password change can be finished without the vault"));
        }
        if core.storage_binding_fingerprint(storage)? != self.binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "plan storage binding changed"));
        }
        if self.steps.iter().any(|step| step.target().storage != storage) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "plan step targets another storage"));
        }
        match next_step(core, self)? {
            Next::Done(status) => Ok(status),
            Next::Submit(step) => {
                let job = self.job_id(step);
                core.submit_with_id(job, self.steps[step].clone())?;
                Ok(CryptomatorPlanStatus::Submitted { step, job })
            }
        }
    }
    pub fn to_json(&self) -> Result<Vec<u8>> { serde_json::to_vec(self).map_err(|_| CryptoError::InvalidInput) }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 4 << 20 { return Err(CryptoError::LimitExceeded); }
        let plan: Self = serde_json::from_slice(bytes).map_err(|_| CryptoError::InvalidInput)?;
        if plan.steps.is_empty() || plan.steps.len() > 4100 { return Err(CryptoError::InvalidInput); }
        for step in &plan.steps { step.validate().map_err(|_| CryptoError::InvalidInput)?; }
        Ok(plan)
    }
}

enum Next { Done(CryptomatorPlanStatus), Submit(usize) }

/// First step that is not Succeeded: still running, stopped, or not yet
/// submitted. Never looks past a step that is not Succeeded.
fn next_step(core: &paranoic_core::Core, plan: &CryptomatorPlan) -> paranoic_core::Result<Next> {
    for (step, operation) in plan.steps.iter().enumerate() {
        let job = plan.job_id(step);
        match core.job(job) {
            Ok(record) if record.operation != *operation =>
                return Err(CoreError::new(ErrorCode::InvalidInput, "plan job ID was used for another request")),
            Ok(record) => match record.state {
                JobState::Succeeded => continue,
                JobState::Queued | JobState::Running => return Ok(Next::Done(CryptomatorPlanStatus::Running { step, job })),
                state => return Ok(Next::Done(CryptomatorPlanStatus::Stopped { step, job, state })),
            },
            Err(error) if error.code == ErrorCode::NotFound => return Ok(Next::Submit(step)),
            Err(error) => return Err(error),
        }
    }
    Ok(Next::Done(CryptomatorPlanStatus::Completed))
}

/// ".pnc-" because ".paranoic-" is reserved for adapter staging (ResourcePath).
/// Not ending in .c9r/.c9s, so readers ignore it as a foreign object.
fn hidden(parent: &ResourcePath, purpose: &str, plan: uuid::Uuid) -> Result<ResourcePath> {
    parent.join(&format!(".pnc-{purpose}-{}", plan.simple())).map_err(|_| CryptoError::InvalidInput)
}
fn join(parent: &ResourcePath, name: &str) -> Result<ResourcePath> {
    parent.join(name).map_err(|_| CryptoError::InvalidInput)
}

impl CryptomatorReader {
    fn plan(&self, kind: CryptomatorPlanKind, plan_id: uuid::Uuid, steps: Vec<Operation>) -> Result<CryptomatorPlan> {
        let binding = self.storage.binding_key().map_err(storage_error)?;
        for step in &steps { step.validate().map_err(|_| CryptoError::InvalidInput)?; }
        Ok(CryptomatorPlan { plan_id, vault_id: self.session.id()?, root: self.root.clone(),
            binding: blake3::hash(binding.as_bytes()).to_hex().to_string(), kind, steps })
    }

    fn location(&self, path: ResourcePath) -> Location { Location { storage: self.storage.id(), path } }

    fn require_mutable(&self) -> Result<()> {
        let caps = self.storage.capabilities();
        if !caps.write || !caps.delete || !caps.read { return Err(CryptoError::Permissions); }
        // SAF: no directory sync; accepted by the owner (E2.8, directory_sync_waived).
        if !caps.atomic_move_no_replace || !(caps.durable_directory_sync || caps.directory_sync_waived) { return Err(CryptoError::Unsupported); }
        Ok(())
    }

    fn require_absent(&self, path: &ResourcePath) -> Result<()> {
        match self.storage.metadata(path) {
            Ok(_) => Err(CryptoError::AlreadyExists),
            Err(error) if error.code == ErrorCode::NotFound => Ok(()),
            Err(error) => Err(storage_error(error)),
        }
    }

    /// Bounded breadth-first walk of linked data directories below `start`.
    /// Returns (data path, depth below start) with `start` first. Aliased or
    /// cyclic links inside the subtree are rejected. Aliases from outside the
    /// subtree cannot be detected without a whole-vault scan.
    pub(crate) fn subtree_directories<F: FnMut() -> Result<()>>(&self, start: &ResourcePath,
        limits: CryptomatorTreeLimits, check: &mut F) -> Result<Vec<(ResourcePath, usize)>>
    {
        if limits.max_directories == 0 || limits.max_entries_per_directory == 0 { return Err(CryptoError::InvalidInput); }
        let mut found = vec![(start.clone(), 0_usize)];
        let mut index = 0;
        while index < found.len() {
            check()?;
            let (path, depth) = found[index].clone();
            index += 1;
            let rows = self.storage.list(&path, limits.max_entries_per_directory).map_err(storage_error)?;
            if rows.len() > limits.max_entries_per_directory { return Err(CryptoError::LimitExceeded); }
            for row in rows {
                let (parent, component) = row.location.path.parent_and_name().map_err(|_| CryptoError::InvalidInput)?;
                if parent != path || row.location.storage != self.storage.id() { return Err(CryptoError::InvalidInput); }
                if row.metadata.kind != EntryKind::Directory || !(component.ends_with(".c9r") || component.ends_with(".c9s")) {
                    continue;
                }
                let (kind, _) = self.wrapper_kind(&row.location.path, component.ends_with(".c9s"), check)?;
                if kind != CryptomatorListedKind::Directory { continue; }
                let link = read_small(self.storage.as_ref(), &join(&row.location.path, "dir.c9r")?, 36, check)?;
                let id = CryptomatorDirectoryId::parse(std::str::from_utf8(&link).map_err(|_| CryptoError::InvalidInput)?)?;
                let child = self.data_path(&id)?;
                if found.iter().any(|(seen, _)| *seen == child) { return Err(CryptoError::InvalidInput); }
                if depth + 1 > 64 { return Err(CryptoError::LimitExceeded); }
                if found.len() >= limits.max_directories { return Err(CryptoError::LimitExceeded); }
                found.push((child, depth + 1));
            }
        }
        Ok(found)
    }

    /// Prepare deletion of a file or a whole directory subtree. The first step
    /// atomically moves the entry to a hidden name in its parent, so it leaves
    /// the visible namespace before any data is removed. Later steps delete the
    /// subtree's data directories (deepest first) and the detached wrapper.
    /// If a later step stops, the remaining detached ciphertext is unreachable
    /// garbage, not a dangling visible entry.
    pub fn prepare_delete<F: FnMut() -> Result<()>>(&self, parent: &[String], name: &str,
        limits: CryptomatorTreeLimits, plan_id: uuid::Uuid, mut checkpoint: F) -> Result<CryptomatorPlan>
    {
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        check()?;
        self.require_mutable()?;
        let (directory, parent_path) = self.resolve_directory(parent, &mut check)?;
        let encoded = self.session.encrypt_name(&directory, name)?;
        let source = join(&parent_path, encoded.component())?;
        let metadata = self.storage.metadata(&source.clone()).map_err(storage_error)?;
        let shortened = encoded.name_mapping().is_some();
        if let Some(expected) = encoded.name_mapping() {
            let actual = read_small(self.storage.as_ref(), &join(&source, "name.c9s")?, 368, &mut check)?;
            if actual.as_slice() != expected.as_bytes() { return Err(CryptoError::Authentication); }
        }
        let steps = match metadata.kind {
            EntryKind::File if !shortened => vec![Operation::DeleteFile { target: self.location(source) }],
            EntryKind::Directory => {
                let (kind, _) = self.wrapper_kind(&source, shortened, &mut check)?;
                let detached = hidden(&parent_path, "delete", plan_id)?;
                self.require_absent(&detached)?;
                let mut steps = vec![Operation::Move { source: self.location(source.clone()), target: self.location(detached.clone()) }];
                if kind == CryptomatorListedKind::Directory {
                    let mut components = parent.to_vec();
                    components.push(name.to_owned());
                    let (_, data) = self.resolve_directory(&components, &mut check)?;
                    let mut directories = self.subtree_directories(&data, limits, &mut check)?;
                    directories.sort_by(|a, b| b.1.cmp(&a.1));
                    for (path, _) in directories {
                        steps.push(Operation::DeleteDirectory { target: self.location(path) });
                    }
                }
                steps.push(Operation::DeleteDirectory { target: self.location(detached) });
                steps
            }
            _ => return Err(CryptoError::Unsupported),
        };
        check()?;
        self.plan(CryptomatorPlanKind::Delete, plan_id, steps)
    }

    /// Prepare a durable password change. Steps: publish the new masterkey file
    /// in a hidden staging directory, rename the current file to
    /// `masterkey.cryptomator.<plan>.bkup`, move the new file into place and
    /// remove the empty staging directory. Between steps 2 and 3 the vault has
    /// no masterkey file; resuming the plan completes it. The backup still
    /// opens the vault with the OLD password until `prepare_remove_masterkey_backup`.
    pub fn prepare_password_change_plan<F: FnMut() -> Result<()>>(&self, password: Zeroizing<String>,
        plan_id: uuid::Uuid, mut checkpoint: F) -> Result<CryptomatorPlan>
    {
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        check()?;
        self.require_mutable()?;
        let current = join(&self.root, "masterkey.cryptomator")?;
        if self.storage.metadata(&current).map_err(storage_error)?.kind != EntryKind::File { return Err(CryptoError::InvalidInput); }
        let staging = hidden(&self.root, "masterkey", plan_id)?;
        let backup = join(&self.root, &format!("masterkey.cryptomator.{}.bkup", plan_id.simple()))?;
        self.require_absent(&staging)?;
        self.require_absent(&backup)?;
        let bytes = self.session.prepare_password_change(password)?;
        check()?;
        let file = PreparedTreeFile { path: ResourcePath::parse("masterkey.cryptomator").map_err(|_| CryptoError::InvalidInput)?, bytes };
        let steps = vec![
            Operation::PublishTree { target: self.location(staging.clone()), files: vec![file] },
            Operation::Move { source: self.location(current.clone()), target: self.location(backup) },
            Operation::Move { source: self.location(join(&staging, "masterkey.cryptomator")?), target: self.location(current) },
            Operation::DeleteEmptyDirectory { target: self.location(staging) },
        ];
        self.plan(CryptomatorPlanKind::PasswordChange, plan_id, steps)
    }

    /// `Ok` only if the stored masterkey file opens THIS vault with
    /// `password` (e.g. the current password before a change). Expensive
    /// key derivation.
    pub fn verify_password<F: FnMut() -> Result<()>>(&self, password: Zeroizing<String>, mut checkpoint: F) -> Result<()> {
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        self.check_password(password, &mut check)
    }
    fn check_password<F: FnMut() -> Result<()>>(&self, password: Zeroizing<String>, check: &mut F) -> Result<()> {
        let keys = read_small(self.storage.as_ref(), &join(&self.root, "masterkey.cryptomator")?, 16384, check)?;
        let vault = if self.session.format()? == 7 {
            CryptomatorVault::unlock_legacy(&keys, password, &mut *check)?
        } else {
            let config = read_small(self.storage.as_ref(), &join(&self.root, "vault.cryptomator")?, 16384, check)?;
            let token = std::str::from_utf8(&config).map_err(|_| CryptoError::InvalidInput)?;
            CryptomatorVault::unlock(&keys, token, password, &mut *check)?
        };
        if vault.id() != self.session.id()? { return Err(CryptoError::Authentication); }
        Ok(())
    }

    /// After a completed PasswordChange plan: verify that the published
    /// masterkey file opens THIS vault with `new_password`, then prepare a
    /// one-step plan deleting the old-password backup. Copies of the old file
    /// outside this storage (cloud versions, backups) are not affected.
    pub fn prepare_remove_masterkey_backup<F: FnMut() -> Result<()>>(&self, completed: &CryptomatorPlan,
        new_password: Zeroizing<String>, plan_id: uuid::Uuid, mut checkpoint: F) -> Result<CryptomatorPlan>
    {
        if completed.kind != CryptomatorPlanKind::PasswordChange || completed.vault_id != self.session.id()?
            || completed.root != self.root || plan_id == completed.plan_id { return Err(CryptoError::InvalidInput); }
        self.require_mutable()?;
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        self.check_password(new_password, &mut check)?;
        let backup = join(&self.root, &format!("masterkey.cryptomator.{}.bkup", completed.plan_id.simple()))?;
        if self.storage.metadata(&backup).map_err(storage_error)?.kind != EntryKind::File { return Err(CryptoError::InvalidInput); }
        check()?;
        self.plan(CryptomatorPlanKind::RemoveMasterkeyBackup, plan_id, vec![Operation::DeleteFile { target: self.location(backup) }])
    }

    /// File rename/move between shortening classes (.c9r <-> .c9s). Content
    /// ciphertext and file header are reused; only the name mapping changes.
    /// While the plan runs the file is hidden in a staging directory; a stopped
    /// plan leaves it there, never deleted. Same-class files and all directory
    /// cases use `prepare_move`/`submit_rename` instead.
    pub fn prepare_file_reshape<F: FnMut() -> Result<()>>(&self, parent: &[String], old_name: &str,
        target_parent: &[String], new_name: &str, plan_id: uuid::Uuid, mut checkpoint: F) -> Result<CryptomatorPlan>
    {
        if parent.len() >= 64 || target_parent.len() >= 64 || (parent == target_parent && old_name == new_name) {
            return Err(CryptoError::InvalidInput);
        }
        let mut check = || { self.session.id()?; checkpoint()?; self.session.id()?; Ok::<(), CryptoError>(()) };
        check()?;
        self.require_mutable()?;
        let (directory, path) = self.resolve_directory(parent, &mut check)?;
        let old = self.session.encrypt_name(&directory, old_name)?;
        let (target_directory, target_path) = self.resolve_directory(target_parent, &mut check)?;
        let new = self.session.encrypt_name(&target_directory, new_name)?;
        let source = join(&path, old.component())?;
        let target = join(&target_path, new.component())?;
        self.require_absent(&target)?;
        let metadata = self.storage.metadata(&source).map_err(storage_error)?;
        let steps = match (old.name_mapping(), new.name_mapping()) {
            (None, Some(mapping)) => {
                if metadata.kind != EntryKind::File { return Err(CryptoError::Unsupported); }
                let staging = hidden(&target_path, "reshape", plan_id)?;
                self.require_absent(&staging)?;
                vec![
                    Operation::PublishTree { target: self.location(staging.clone()), files: vec![PreparedTreeFile {
                        path: ResourcePath::parse("name.c9s").map_err(|_| CryptoError::InvalidInput)?,
                        bytes: mapping.as_bytes().to_vec() }] },
                    Operation::Move { source: self.location(source), target: self.location(join(&staging, "contents.c9r")?) },
                    Operation::Move { source: self.location(staging), target: self.location(target) },
                ]
            }
            (Some(mapping), None) => {
                if metadata.kind != EntryKind::Directory { return Err(CryptoError::Unsupported); }
                let actual = read_small(self.storage.as_ref(), &join(&source, "name.c9s")?, 368, &mut check)?;
                if actual.as_slice() != mapping.as_bytes() { return Err(CryptoError::Authentication); }
                let (kind, _) = self.wrapper_kind(&source, true, &mut check)?;
                if kind != CryptomatorListedKind::File { return Err(CryptoError::Unsupported); }
                let staging = hidden(&path, "reshape", plan_id)?;
                self.require_absent(&staging)?;
                vec![
                    Operation::Move { source: self.location(source), target: self.location(staging.clone()) },
                    Operation::Move { source: self.location(join(&staging, "contents.c9r")?), target: self.location(target) },
                    Operation::DeleteFile { target: self.location(join(&staging, "name.c9s")?) },
                    Operation::DeleteEmptyDirectory { target: self.location(staging) },
                ]
            }
            _ => return Err(CryptoError::InvalidInput),
        };
        check()?;
        self.plan(CryptomatorPlanKind::ReshapeFile, plan_id, steps)
    }

    /// Submit the next step of `plan`. Never submits past a step that is not
    /// Succeeded. Requires the same unlocked vault, root and storage binding.
    pub fn advance_plan(&self, core: &paranoic_core::Core, plan: &CryptomatorPlan)
        -> paranoic_core::Result<CryptomatorPlanStatus>
    {
        let vault_id = self.session.id().map_err(|_| CoreError::new(ErrorCode::PermissionDenied, "vault session locked"))?;
        if vault_id != plan.vault_id || self.root != plan.root {
            return Err(CoreError::new(ErrorCode::InvalidInput, "plan belongs to another vault root"));
        }
        let binding = self.storage.binding_key()?;
        if blake3::hash(binding.as_bytes()).to_hex().as_str() != plan.binding
            || core.storage_binding_fingerprint(self.storage.id())? != plan.binding {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "plan storage binding changed"));
        }
        if plan.steps.iter().any(|operation| operation.target().storage != self.storage.id()) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "plan step targets another storage"));
        }
        match next_step(core, plan)? {
            Next::Done(status) => Ok(status),
            Next::Submit(step) => {
                if plan.kind == CryptomatorPlanKind::Delete && step == 1 && plan.steps.len() > 2 {
                    self.recheck_detached_subtree(plan)?;
                }
                let job = plan.job_id(step);
                core.submit_with_id(job, plan.steps[step].clone())?;
                Ok(CryptomatorPlanStatus::Submitted { step, job })
            }
        }
    }

    /// Before the first data deletion, the detached subtree must still consist
    /// of exactly the planned data directories: a child moved out after
    /// preparation must never be deleted through a stale plan.
    fn recheck_detached_subtree(&self, plan: &CryptomatorPlan) -> paranoic_core::Result<()> {
        let planned: Vec<&ResourcePath> = plan.steps[1..plan.steps.len() - 1].iter()
            .map(|step| match step {
                Operation::DeleteDirectory { target } => Ok(&target.path),
                _ => Err(CoreError::new(ErrorCode::InvalidInput, "unexpected delete plan step")),
            }).collect::<paranoic_core::Result<_>>()?;
        let start = planned.last().copied().ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "empty delete plan"))?;
        let limits = CryptomatorTreeLimits { max_directories: planned.len().max(1), ..CryptomatorTreeLimits::default() };
        let current = self.subtree_directories(start, limits, &mut || Ok(()))
            .map_err(|_| CoreError::new(ErrorCode::SourceChanged, "detached directory subtree changed"))?;
        if current.len() != planned.len() || current.iter().any(|(path, _)| !planned.contains(&path)) {
            return Err(CoreError::new(ErrorCode::SourceChanged, "detached directory subtree changed"));
        }
        Ok(())
    }
}
