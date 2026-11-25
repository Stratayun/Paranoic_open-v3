//! Backup and two-way sync between a local folder and a cloud folder, both
//! registered in Core (roadmap 3.8). Planning is pure observation; every
//! change is an ordinary journaled Core job; the baseline advances only for
//! jobs that succeeded. Safety rules:
//! * conflicts are never auto-resolved: the cloud version is downloaded next
//!   to the local file as a KeepBoth copy ("name (1).ext"), nothing is replaced;
//! * local files are never deleted by sync; cloud deletions are opt-in and go
//!   to the provider trash;
//! * replacing a file (Core ConflictPolicy::Replace) happens only when both
//!   sides still match the planned observation right before submission;
//! * equal content found without a baseline (first run, crash recovery) is
//!   adopted after a SHA-256 comparison when the server reports one; unknown
//!   equality is treated as a conflict;
//! * the baseline records the versions a job actually synchronized, never a
//!   later observation, so an edit made after the job stays a change.
use std::{collections::{BTreeMap, BTreeSet}, io::Read, sync::atomic::AtomicBool};
use paranoic_core::{Core, EntryKind, JobId, JobState, Location, Metadata, Operation, ResourcePath, TreeLimits};
use paranoic_crypto::SecretKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CloudStorage, CoreError, ErrorCode, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Local is the source: new/changed local files go to the cloud; cloud-only
    /// or cloud-modified files are reported, never downloaded over local data.
    Backup,
    TwoWay,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SyncOptions { pub mode: SyncMode, /** Send files deleted locally to the cloud trash. */ pub trash_remote_deletions: bool }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Side { bytes: u64, modified: Option<u64> }
impl From<&Metadata> for Side { fn from(m: &Metadata) -> Self { Self { bytes: m.bytes, modified: m.modified_unix_ms } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct BaseEntry { directory: bool, local: Side, remote: Side,
    /// Provider revision (ETag) of the remote file when last synchronized:
    /// a different revision is a change even with equal size and mtime (У-24).
    #[serde(default)]
    remote_revision: Option<String> }

/// Last synchronized state. Contains file names: persist only sealed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncBaseline { version: u8, local: Location, remote: Location, entries: BTreeMap<String, BaseEntry>,
    /// Paths with an unresolved conflict (cloud copy saved locally). They are
    /// reported, never synchronized, until `resolve_conflict` records a choice.
    #[serde(default)]
    conflicts: BTreeSet<String> }

/// Explicit user decision for a conflicting path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictChoice { KeepLocal, KeepRemote }

/// Planned action with the observed local and remote versions.
type Decision = (SyncActionKind, Option<Side>, Option<Side>);

/// Side marker that never equals an observation: forces the chosen direction.
const STALE: Side = Side { bytes: u64::MAX, modified: None };
fn file_meta(side: Side) -> Metadata { Metadata { kind: EntryKind::File, bytes: side.bytes, modified_unix_ms: side.modified } }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncActionKind { CreateLocalFolder, CreateRemoteFolder, Upload, UploadReplace, Download, DownloadReplace, ConflictCopy, TrashRemote, Adopt }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncAction {
    pub path: String,
    pub kind: SyncActionKind,
    /// Persist the plan before `submit`; a job id already in the journal is never submitted twice.
    pub job: Option<JobId>,
    pub operation: Option<Operation>,
    expect_local: Option<Side>,
    expect_remote: Option<Side>,
    /// Provider revision of the remote file at planning time (download source,
    /// adopt): what the baseline records, not a later observation (У-36).
    #[serde(default)]
    expect_revision: Option<String>,
    submitted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncReport {
    Conflict { path: String },
    DeletedLocally { path: String },
    DeletedRemotely { path: String },
    RemoteOnly { path: String },
    RemoteModified { path: String },
    TypeMismatch { path: String },
    Unsupported { path: String },
    ChangedBeforeSubmit { path: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncPlan { pub actions: Vec<SyncAction>, pub reports: Vec<SyncReport> }

fn context(local: &Location, remote: &Location) -> Result<Vec<u8>> {
    let mut out = b"paranoic/sync-baseline/v1/".to_vec();
    out.extend(serde_json::to_vec(&(local, remote)).map_err(|_| CoreError::new(ErrorCode::Internal, "baseline context"))?);
    Ok(out)
}
fn under(root: &Location, path: &ResourcePath) -> Result<ResourcePath> {
    if root.path.is_root() { Ok(path.clone()) } else { ResourcePath::parse(format!("{}/{}", root.path.as_str(), path.as_str())) }
}
/// Service objects never take part in sync: Paranoic spools and staging
/// (`.pnc-…`, may hold decrypted data during export or extraction) and
/// adapter staging (`.paranoic-…`). External audit A-1.
pub fn is_service_path(relative: &str) -> bool {
    relative.split('/').any(|part| part.starts_with(".pnc-") || part.starts_with(".paranoic-"))
}
fn relative(root: &Location, path: &ResourcePath) -> Option<String> {
    if root.path.is_root() { return (!path.is_root()).then(|| path.as_str().to_owned()); }
    path.as_str().strip_prefix(root.path.as_str())?.strip_prefix('/').map(str::to_owned)
}

impl SyncBaseline {
    pub fn new(local: Location, remote: Location) -> Self { Self { version: 1, local, remote, entries: BTreeMap::new(), conflicts: BTreeSet::new() } }
    pub fn conflicts(&self) -> impl Iterator<Item = &str> { self.conflicts.iter().map(String::as_str) }
    /// Record the user's choice for a conflicting file from fresh observations.
    /// The next plan replaces the other side, version-checked when the job runs.
    pub fn resolve_conflict(&mut self, core: &Core, path: &str, choice: ConflictChoice) -> Result<()> {
        if !self.conflicts.contains(path) { return Err(CoreError::new(ErrorCode::NotFound, "no conflict recorded for this path")); }
        let rel = ResourcePath::parse(path)?;
        let local = Side::from(&core.metadata(&Location { storage: self.local.storage, path: under(&self.local, &rel)? })?);
        let remote = Side::from(&core.metadata(&Location { storage: self.remote.storage, path: under(&self.remote, &rel)? })?);
        let entry = match choice {
            ConflictChoice::KeepLocal => BaseEntry { directory: false, local: STALE, remote, remote_revision: None },
            ConflictChoice::KeepRemote => BaseEntry { directory: false, local, remote: STALE, remote_revision: None },
        };
        self.entries.insert(path.to_owned(), entry);
        self.conflicts.remove(path);
        Ok(())
    }
    pub fn local(&self) -> &Location { &self.local }
    pub fn remote(&self) -> &Location { &self.remote }
    /// Accept a reported deletion/conflict state: forget the path (and below).
    pub fn forget(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.entries.retain(|k, _| k != path && !k.starts_with(&prefix));
    }
    pub fn seal(&self, key: &SecretKey) -> Result<Vec<u8>> {
        let plain = Zeroizing::new(serde_json::to_vec(self).map_err(|_| CoreError::new(ErrorCode::Internal, "baseline encoding failed"))?);
        key.seal(&plain, &context(&self.local, &self.remote)?).map_err(|_| CoreError::new(ErrorCode::LimitExceeded, "baseline too large to seal"))
    }
    pub fn open(key: &SecretKey, sealed: &[u8], local: &Location, remote: &Location) -> Result<Self> {
        let plain = key.open(sealed, &context(local, remote)?).map_err(|_| CoreError::new(ErrorCode::Integrity, "sync baseline invalid or for another pair"))?;
        let baseline: Self = serde_json::from_slice(&plain).map_err(|_| CoreError::new(ErrorCode::Integrity, "sync baseline malformed"))?;
        if baseline.version != 1 || baseline.local != *local || baseline.remote != *remote { return Err(CoreError::new(ErrorCode::Integrity, "sync baseline mismatch")); }
        Ok(baseline)
    }

    /// Observe both trees and compute actions. No side effects.
    pub fn plan(&self, core: &Core, cloud: &CloudStorage, options: SyncOptions, cancelled: &AtomicBool) -> Result<SyncPlan> {
        use paranoic_core::Storage;
        if self.remote.storage != cloud.id() || self.local.storage == self.remote.storage {
            return Err(CoreError::new(ErrorCode::InvalidInput, "sync pair does not match the cloud grant"));
        }
        let scan = |root: &Location| -> Result<BTreeMap<String, Metadata>> {
            let inventory = core.scan_tree(root, TreeLimits::default(), cancelled)?;
            Ok(inventory.entries.into_iter().filter_map(|e| relative(root, &e.location.path).map(|p| (p, e.metadata)))
                .filter(|(p, _)| !is_service_path(p)).collect())
        };
        let local = scan(&self.local)?;
        let remote = scan(&self.remote)?;
        let revisions = cloud.revisions(&self.remote.path, TreeLimits::default().max_entries)?;
        // Changed remotely: size/mtime differ, or both revisions known and different.
        let remote_changed = |path: &str, now: &Metadata, base: &BaseEntry| {
            Side::from(now) != base.remote
                || matches!((base.remote_revision.as_deref(), revisions.get(path)), (Some(old), Some(new)) if old != new)
        };
        let mut plan = SyncPlan { actions: Vec::new(), reports: Vec::new() };
        let mut blocked: Vec<String> = Vec::new();
        let is_blocked = |blocked: &[String], path: &str| blocked.iter().any(|b| path == b || path.starts_with(&format!("{b}/")));
        let paths: BTreeSet<&String> = local.keys().chain(remote.keys()).chain(self.entries.keys()).collect();
        let two_way = options.mode == SyncMode::TwoWay;
        let mut need_local_dirs: BTreeSet<String> = BTreeSet::new();
        let mut need_remote_dirs: BTreeSet<String> = BTreeSet::new();
        for path in paths {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) { return Err(CoreError::new(ErrorCode::Cancelled, "sync planning cancelled")); }
            if is_blocked(&blocked, path) { continue; }
            if self.conflicts.contains(path.as_str()) {
                plan.reports.push(SyncReport::Conflict { path: path.clone() }); continue;
            }
            let (l, r, b) = (local.get(path), remote.get(path), self.entries.get(path));
            let kind = |m: &Metadata| m.kind;
            if l.is_some_and(|m| !matches!(kind(m), EntryKind::File | EntryKind::Directory)) || r.is_some_and(|m| !matches!(kind(m), EntryKind::File | EntryKind::Directory)) {
                plan.reports.push(SyncReport::Unsupported { path: path.clone() }); blocked.push(path.clone()); continue;
            }
            if let (Some(lm), Some(rm)) = (l, r) {
                if lm.kind != rm.kind { plan.reports.push(SyncReport::TypeMismatch { path: path.clone() }); blocked.push(path.clone()); continue; }
            }
            if l.is_none() && r.is_none() {
                // Gone on both sides: commit drops the baseline entry.
                if b.is_some() { plan.actions.push(self.make(path, SyncActionKind::Adopt, None, None, None)?); }
                continue;
            }
            let directory = l.or(r).is_some_and(|m| m.kind == EntryKind::Directory) || (l.is_none() && r.is_none() && b.is_some_and(|e| e.directory));
            let action = |kind, l: Option<&Metadata>, r: Option<&Metadata>| (kind, l.map(Side::from), r.map(Side::from));
            let decided: Option<(SyncActionKind, Option<Side>, Option<Side>)> = if directory {
                match (l, r, b) {
                    (Some(_), Some(_), None) => Some(action(SyncActionKind::Adopt, l, r)),
                    (Some(_), None, None) => { need_remote_dirs.insert(path.clone()); None }
                    (None, Some(_), None) if two_way => { need_local_dirs.insert(path.clone()); None }
                    (None, Some(_), None) => { plan.reports.push(SyncReport::RemoteOnly { path: path.clone() }); None }
                    (Some(_), None, Some(_)) => { if two_way { plan.reports.push(SyncReport::DeletedRemotely { path: path.clone() }); } else { need_remote_dirs.insert(path.clone()); } None }
                    (None, Some(_), Some(_)) => { plan.reports.push(SyncReport::DeletedLocally { path: path.clone() }); None }
                    _ => None,
                }
            } else {
                match (l, r, b) {
                    (Some(lm), Some(rm), Some(base)) => {
                        let (lc, rc) = (Side::from(lm) != base.local, remote_changed(path, rm, base));
                        match (lc, rc) {
                            (false, false) => None,
                            (true, false) => Some(action(SyncActionKind::UploadReplace, l, r)),
                            (false, true) if two_way => Some(action(SyncActionKind::DownloadReplace, l, r)),
                            (false, true) => { plan.reports.push(SyncReport::RemoteModified { path: path.clone() }); None }
                            (true, true) => self.equal_or_conflict(core, cloud, path, lm, rm, two_way, &mut plan)?,
                        }
                    }
                    (Some(lm), Some(rm), None) => self.equal_or_conflict(core, cloud, path, lm, rm, two_way, &mut plan)?,
                    (Some(lm), None, base) => {
                        let changed = base.is_none_or(|b| Side::from(lm) != b.local);
                        if changed || !two_way { Some(action(SyncActionKind::Upload, l, None)) }
                        else { plan.reports.push(SyncReport::DeletedRemotely { path: path.clone() }); None }
                    }
                    (None, Some(rm), base) => match base {
                        None if two_way => Some(action(SyncActionKind::Download, None, r)),
                        None => { plan.reports.push(SyncReport::RemoteOnly { path: path.clone() }); None }
                        Some(b) if remote_changed(path, rm, b) => {
                            if two_way { Some(action(SyncActionKind::Download, None, r)) }
                            else { plan.reports.push(SyncReport::RemoteModified { path: path.clone() }); None }
                        }
                        Some(_) if options.trash_remote_deletions => Some(action(SyncActionKind::TrashRemote, None, r)),
                        Some(_) => { plan.reports.push(SyncReport::DeletedLocally { path: path.clone() }); None }
                    },
                    (None, None, _) => None,
                }
            };
            let Some((kind, expect_local, expect_remote)) = decided else { continue };
            let parent = path.rsplit_once('/').map(|(p, _)| p.to_owned());
            match kind {
                SyncActionKind::Upload | SyncActionKind::UploadReplace => if let Some(p) = &parent { if !remote.contains_key(p) { need_remote_dirs.insert(p.clone()); } },
                SyncActionKind::Download | SyncActionKind::DownloadReplace | SyncActionKind::ConflictCopy => if let Some(p) = &parent { if !local.contains_key(p) { need_local_dirs.insert(p.clone()); } },
                _ => {}
            }
            plan.actions.push(self.make(path, kind, expect_local, expect_remote, revisions.get(path.as_str()).cloned())?);
        }
        // Folder creations (all ancestors, parents first) precede file jobs; Core runs jobs in submission order.
        let mut folders = Vec::new();
        for (set, kind, present) in [(need_remote_dirs, SyncActionKind::CreateRemoteFolder, &remote), (need_local_dirs, SyncActionKind::CreateLocalFolder, &local)] {
            let mut all = BTreeSet::new();
            for dir in set {
                let mut current = String::new();
                for part in dir.split('/') {
                    current = if current.is_empty() { part.to_owned() } else { format!("{current}/{part}") };
                    if !present.contains_key(&current) { all.insert(current.clone()); }
                }
            }
            for dir in all { if !is_blocked(&blocked, &dir) { folders.push(self.make(&dir, kind, None, None, None)?); } }
        }
        folders.sort_by(|a, b| a.path.split('/').count().cmp(&b.path.split('/').count()).then(a.path.cmp(&b.path)));
        folders.append(&mut plan.actions);
        plan.actions = folders;
        Ok(plan)
    }

    /// Backup mode only reports a conflict (no download into the local tree).
    #[allow(clippy::too_many_arguments)]
    fn equal_or_conflict(&self, core: &Core, cloud: &CloudStorage, path: &str, lm: &Metadata, rm: &Metadata, two_way: bool, plan: &mut SyncPlan)
        -> Result<Option<Decision>> {
        let (l, r) = (Some(Side::from(lm)), Some(Side::from(rm)));
        if lm.bytes == rm.bytes && same_content(core, cloud, &self.local, &self.remote, path)? == Some(true) {
            return Ok(Some((SyncActionKind::Adopt, l, r)));
        }
        plan.reports.push(SyncReport::Conflict { path: path.to_owned() });
        Ok(two_way.then_some((SyncActionKind::ConflictCopy, l, r)))
    }

    /// `revision`: the remote provider revision seen by this plan; the job
    /// checks it on the remote side (source of a download, target of a replace).
    fn make(&self, path: &str, kind: SyncActionKind, expect_local: Option<Side>, expect_remote: Option<Side>, revision: Option<String>) -> Result<SyncAction> {
        let rel = ResourcePath::parse(path)?;
        let local = Location { storage: self.local.storage, path: under(&self.local, &rel)? };
        let remote = Location { storage: self.remote.storage, path: under(&self.remote, &rel)? };
        // Every write carries the planned versions; Core re-checks them when
        // the job runs, so a change while queued is a conflict, not an overwrite.
        let need = |side: Option<Side>| side.map(file_meta).ok_or_else(|| CoreError::new(ErrorCode::Internal, "sync action without observation"));
        let copy = |source: Location, target: Location, from: Option<Side>, over: Option<Side>, keep_both: bool,
            (expected_source_revision, expected_target_revision): (Option<String>, Option<String>)| -> Result<Option<Operation>> {
            Ok(Some(Operation::CopyFileExpected { source, target, expected_source: need(from)?, expected_target: over.map(file_meta), keep_both,
                expected_source_revision, expected_target_revision }))
        };
        let (up, down) = ((None, revision.clone()), (revision.clone(), None));
        let operation = match kind {
            SyncActionKind::Adopt => None,
            SyncActionKind::CreateLocalFolder => Some(Operation::CreateDirectory { target: local }),
            SyncActionKind::CreateRemoteFolder => Some(Operation::CreateDirectory { target: remote }),
            SyncActionKind::Upload => copy(local, remote, expect_local, None, false, (None, None))?,
            SyncActionKind::UploadReplace => copy(local, remote, expect_local, Some(need(expect_remote).map(|m| Side::from(&m))?), false, up)?,
            SyncActionKind::Download => copy(remote, local, expect_remote, None, false, down.clone())?,
            SyncActionKind::DownloadReplace => copy(remote, local, expect_remote, Some(need(expect_local).map(|m| Side::from(&m))?), false, down.clone())?,
            SyncActionKind::ConflictCopy => copy(remote, local, expect_remote, None, true, down)?,
            SyncActionKind::TrashRemote => Some(Operation::DeleteFileExpected { target: remote, expected: need(expect_remote)?,
                expected_revision: revision.clone() }),
        };
        Ok(SyncAction { path: path.to_owned(), kind, job: operation.as_ref().map(|_| JobId(uuid::Uuid::new_v4())), operation, expect_local, expect_remote,
            expect_revision: revision, submitted: false })
    }

    /// Submit planned jobs in order. Each action is re-checked against its
    /// observation first; a changed path is reported and skipped (re-planned
    /// next run). Job ids already present in the journal are not resubmitted.
    /// The remote provider revision is re-checked too: Core jobs compare only
    /// size and time, and a same-size edit within the same second is invisible
    /// to them (У-24). The job itself checks the same revision again (Core
    /// `expected_*_revision`); this early check only reports the change sooner.
    pub fn submit(&self, core: &Core, cloud: &CloudStorage, plan: &mut SyncPlan) -> Result<()> {
        let pending = plan.actions.iter().any(|a| a.job.is_some() && !a.submitted && a.expect_revision.is_some());
        let revisions = if pending { cloud.revisions(&self.remote.path, TreeLimits::default().max_entries)? } else { BTreeMap::new() };
        for action in &mut plan.actions {
            let (Some(job), Some(operation)) = (action.job, action.operation.clone()) else { continue };
            if action.submitted || core.job(job).is_ok() { action.submitted = true; continue; }
            let rel = ResourcePath::parse(action.path.as_str())?;
            let observe = |root: &Location| -> Result<Option<Side>> {
                match core.metadata(&Location { storage: root.storage, path: under(root, &rel)? }) {
                    Ok(m) => Ok(Some(Side::from(&m))),
                    Err(e) if e.code == ErrorCode::NotFound => Ok(None),
                    Err(e) => Err(e),
                }
            };
            let folder = matches!(action.kind, SyncActionKind::CreateLocalFolder | SyncActionKind::CreateRemoteFolder);
            let revision_changed = action.expect_revision.is_some() && revisions.get(action.path.as_str()) != action.expect_revision.as_ref();
            if !folder && (observe(&self.local)? != action.expect_local || observe(&self.remote)? != action.expect_remote || revision_changed) {
                plan.reports.push(SyncReport::ChangedBeforeSubmit { path: action.path.clone() });
                action.job = None; action.operation = None;
                continue;
            }
            core.submit_with_id(job, operation)?;
            action.submitted = true;
        }
        Ok(())
    }

    /// Advance the baseline for finished actions only (Adopt, or job Succeeded).
    /// The baseline records the versions that were actually synchronized, never
    /// a later observation (У-36): an edit made after the job ran must stay
    /// visible as a change to the next plan.
    /// * source side of a copy: the planned version, which Core verified before
    ///   and after copying; if the source changed since, the next plan sees it;
    /// * target side: recorded only if it still holds what the job wrote
    ///   (length; for a local target also BLAKE3 of the content);
    /// * Adopt (equal content found at planning): recorded only if both sides
    ///   still match the planned observations.
    ///
    /// Anything that does not match is left out and planned again next run.
    pub fn commit(&mut self, core: &Core, cloud: &CloudStorage, plan: &SyncPlan) -> Result<()> {
        for action in &plan.actions {
            let record = match action.job {
                None if action.kind == SyncActionKind::Adopt => None,
                None => continue,
                Some(job) => match core.job(job) {
                    Ok(j) if j.state == JobState::Succeeded => Some(j),
                    _ => continue,
                },
            };
            let rel = ResourcePath::parse(action.path.as_str())?;
            let local_at = Location { storage: self.local.storage, path: under(&self.local, &rel)? };
            let remote_at = Location { storage: self.remote.storage, path: under(&self.remote, &rel)? };
            if action.kind == SyncActionKind::TrashRemote { self.entries.remove(&action.path); continue; }
            if action.kind == SyncActionKind::ConflictCopy {
                // The original pair still differs: keep it out of the baseline
                // until the user resolves it (resolve_conflict).
                self.entries.remove(&action.path);
                self.conflicts.insert(action.path.clone());
                continue;
            }
            let (local_now, remote_now) = (core.metadata(&local_at), core.metadata(&remote_at));
            if let (Err(l), Err(r)) = (&local_now, &remote_now) {
                if l.code == ErrorCode::NotFound && r.code == ErrorCode::NotFound && action.kind == SyncActionKind::Adopt {
                    self.forget(&action.path);
                }
                continue;
            }
            let (Ok(l), Ok(r)) = (local_now, remote_now) else { continue };
            if l.kind != r.kind { continue; }
            if l.kind == EntryKind::Directory {
                // Folder versions are not compared by planning; presence is what counts.
                self.entries.insert(action.path.clone(), BaseEntry { directory: true, local: Side::from(&l), remote: Side::from(&r), remote_revision: None });
                continue;
            }
            let written = |now: &Metadata| record.as_ref().and_then(|j| j.result.as_ref()).is_some_and(|result| result.bytes == now.bytes);
            let entry = match action.kind {
                SyncActionKind::Adopt => {
                    if Some(Side::from(&l)) != action.expect_local || Some(Side::from(&r)) != action.expect_remote { continue; }
                    let revision = cloud.revision(&remote_at.path)?;
                    if action.expect_revision.is_some() && revision != action.expect_revision { continue; }
                    BaseEntry { directory: false, local: Side::from(&l), remote: Side::from(&r), remote_revision: revision }
                }
                SyncActionKind::Upload | SyncActionKind::UploadReplace => {
                    let Some(source) = action.expect_local else { continue };
                    if !written(&r) { continue; }
                    // The revision our upload produced, read by the job right
                    // after publishing; not whatever is there now (audit A-5).
                    // If the remote moved on since, nothing is recorded and
                    // the next plan sees the change.
                    let uploaded = record.as_ref().and_then(|j| j.result.as_ref()).and_then(|result| result.revision.clone());
                    let now = cloud.revision(&remote_at.path)?;
                    if uploaded.is_some() && now != uploaded { continue; }
                    BaseEntry { directory: false, local: source, remote: Side::from(&r), remote_revision: uploaded.or(now) }
                }
                SyncActionKind::Download | SyncActionKind::DownloadReplace => {
                    let Some(source) = action.expect_remote else { continue };
                    if !written(&l) || !local_matches(core, &local_at, record.as_ref())? { continue; }
                    BaseEntry { directory: false, local: Side::from(&l), remote: source, remote_revision: action.expect_revision.clone() }
                }
                _ => continue,
            };
            self.entries.insert(action.path.clone(), entry);
        }
        Ok(())
    }
}

/// The local file still holds exactly the bytes the job streamed (BLAKE3).
fn local_matches(core: &Core, at: &Location, record: Option<&paranoic_core::JobRecord>) -> Result<bool> {
    let Some(expected) = record.and_then(|j| j.result.as_ref()).and_then(|r| r.blake3.clone()) else { return Ok(false) };
    let mut input = core.open_read(at)?;
    let mut hash = blake3::Hasher::new();
    let mut buffer = Zeroizing::new(vec![0u8; 1 << 16]);
    loop {
        let n = input.read(&mut buffer).map_err(CoreError::io)?;
        if n == 0 { break; }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_hex().as_str() == expected)
}

/// Compare a local file with the remote one. None = equality unknown.
/// SHA-256 from the server is accepted as proof.
fn same_content(core: &Core, cloud: &CloudStorage, local: &Location, remote: &Location, path: &str) -> Result<Option<bool>> {
    let rel = ResourcePath::parse(path)?;
    let remote_at = Location { storage: remote.storage, path: under(remote, &rel)? };
    let entry = cloud.remote_entry(&remote_at.path)?;
    let Some(expected) = entry.sha256 else { return Ok(None) };
    let mut sha = Sha256::new();
    read_all(core, &Location { storage: local.storage, path: under(local, &rel)? }, |data| sha.update(data))?;
    let digest: [u8; 32] = sha.finalize().into();
    Ok(Some(expected == digest))
}

fn read_all(core: &Core, at: &Location, mut each: impl FnMut(&[u8])) -> Result<()> {
    let mut input = core.open_read(at)?;
    let mut buffer = Zeroizing::new(vec![0u8; 1 << 16]);
    loop {
        let n = input.read(&mut buffer).map_err(CoreError::io)?;
        if n == 0 { return Ok(()); }
        each(&buffer[..n]);
    }
}
