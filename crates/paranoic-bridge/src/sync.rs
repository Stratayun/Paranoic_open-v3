//! Backup and two-way sync between a local folder and a folder of a connected
//! cloud (roadmap 3.8, plan B4) on top of `paranoic_cloud::SyncBaseline`.
//! State (pairs, baselines with file names, the pending plan) is one record
//! sealed with the host record key (Keystore / DPAPI), never stored in clear;
//! without that key sync is unavailable, like saved cloud credentials.
//! A plan is persisted before it is submitted, so a run that stopped is run
//! again without repeating a job (Core knows the job IDs). Conflicts are never
//! resolved automatically: the user chooses per path.
use crate::*;
use paranoic_cloud::{ConflictChoice, SyncActionKind, SyncBaseline, SyncMode, SyncOptions, SyncPlan, SyncReport};
use serde::{Deserialize, Serialize};

/// `mode`: 1 backup (local → cloud, cloud changes only reported), 2 two-way.
/// `conflicts`: paths waiting for a decision (`sync_resolve`).
#[derive(Clone, uniffi::Record)]
pub struct NativeSyncPair { pub id: String, pub local: NativeLocation, pub remote: NativeLocation, pub mode: u8,
    pub trash_remote_deletions: bool, pub conflicts: Vec<String> }
/// `kind` of an action: 1 create local folder, 2 create cloud folder, 3 upload,
/// 4 upload replacing, 5 download, 6 download replacing, 7 conflict copy (the
/// cloud version saved next to the local file), 8 move to cloud trash, 9 adopt
/// (equal content). Of a report: 1 conflict, 2 deleted locally, 3 deleted in
/// the cloud, 4 only in the cloud, 5 changed in the cloud, 6 file/folder type
/// differs, 7 unsupported entry, 8 changed before it could be submitted.
#[derive(Clone, uniffi::Record)]
pub struct NativeSyncItem { pub path: String, pub kind: u8 }
#[derive(Clone, uniffi::Record)]
pub struct NativeSyncPlan { pub actions: Vec<NativeSyncItem>, pub reports: Vec<NativeSyncItem> }
/// Outcome of `sync_run`: jobs that succeeded / did not, and the reports.
#[derive(Clone, uniffi::Record)]
pub struct NativeSyncResult { pub done: u32, pub failed: u32, pub reports: Vec<NativeSyncItem> }

#[derive(Clone, Serialize, Deserialize)]
struct Pair { id: String, mode: SyncMode, trash_remote_deletions: bool, baseline: SyncBaseline, plan: Option<SyncPlan> }
#[derive(Default, Serialize, Deserialize)]
struct State { version: u8, pairs: Vec<Pair> }

const STATE_FILE: &str = "sync-state";
const MAX_STATE: u64 = 64 << 20;
const MAX_PAIRS: usize = 64;
/// One sync operation at a time (state read-modify-write, long runs).
static SYNC: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn action_code(kind: SyncActionKind) -> u8 {
    match kind {
        SyncActionKind::CreateLocalFolder => 1, SyncActionKind::CreateRemoteFolder => 2, SyncActionKind::Upload => 3,
        SyncActionKind::UploadReplace => 4, SyncActionKind::Download => 5, SyncActionKind::DownloadReplace => 6,
        SyncActionKind::ConflictCopy => 7, SyncActionKind::TrashRemote => 8, SyncActionKind::Adopt => 9,
    }
}
fn report(item: &SyncReport) -> NativeSyncItem {
    let (path, kind) = match item {
        SyncReport::Conflict { path } => (path, 1), SyncReport::DeletedLocally { path } => (path, 2),
        SyncReport::DeletedRemotely { path } => (path, 3), SyncReport::RemoteOnly { path } => (path, 4),
        SyncReport::RemoteModified { path } => (path, 5), SyncReport::TypeMismatch { path } => (path, 6),
        SyncReport::Unsupported { path } => (path, 7), SyncReport::ChangedBeforeSubmit { path } => (path, 8),
    };
    NativeSyncItem { path: path.clone(), kind }
}
fn summary(plan: &SyncPlan) -> NativeSyncPlan {
    NativeSyncPlan {
        actions: plan.actions.iter().filter(|a| a.job.is_some() || a.kind == SyncActionKind::Adopt)
            .map(|a| NativeSyncItem { path: a.path.clone(), kind: action_code(a.kind) }).collect(),
        reports: plan.reports.iter().map(report).collect(),
    }
}
fn native(pair: &Pair) -> NativeSyncPair {
    NativeSyncPair { id: pair.id.clone(), local: pair.baseline.local().clone().into(), remote: pair.baseline.remote().clone().into(),
        mode: if pair.mode == SyncMode::Backup { 1 } else { 2 }, trash_remote_deletions: pair.trash_remote_deletions,
        conflicts: pair.baseline.conflicts().map(str::to_owned).collect() }
}

impl NativeRuntime {
    fn sync_guard(&self) -> Result<std::sync::MutexGuard<'static, ()>> {
        if !self.sealer.available() { return Err(BridgeError::DeviceKey); }
        SYNC.lock().map_err(lock_error)
    }
    fn sync_load(&self) -> Result<State> {
        let path = self.state_root.join(STATE_FILE);
        let sealed = match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_STATE => return Err(BridgeError::Limit),
            Ok(_) => std::fs::read(&path).map_err(|_| BridgeError::Unavailable)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(State { version: 1, pairs: Vec::new() }),
            Err(_) => return Err(BridgeError::Unavailable),
        };
        let opened = self.sealer.open(sealed);
        if opened.error != 0 { return Err(BridgeError::DeviceKey); }
        let plain = zeroize::Zeroizing::new(opened.data);
        let state: State = serde_json::from_slice(&plain).map_err(|_| BridgeError::Changed)?;
        if state.version != 1 { return Err(BridgeError::Unsupported); }
        Ok(state)
    }
    /// Sealed, written to a temporary file and renamed over the old state.
    fn sync_store(&self, state: &State) -> Result<()> {
        let plain = zeroize::Zeroizing::new(serde_json::to_vec(state).map_err(|_| BridgeError::Invalid)?);
        let sealed = self.sealer.seal(plain.to_vec());
        if sealed.error != 0 { return Err(BridgeError::DeviceKey); }
        // Audit A09: never write a state that the reader would refuse.
        if sealed.data.len() as u64 > MAX_STATE { return Err(BridgeError::Limit); }
        let path = self.state_root.join(STATE_FILE);
        let temporary = path.with_extension("tmp");
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&temporary)?;
            file.write_all(&sealed.data)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            // Audit A08: the rename itself must survive power loss (directory entry flushed).
            #[cfg(unix)] { std::fs::File::open(&self.state_root)?.sync_all()?; }
            Ok(())
        };
        write().map_err(|_| BridgeError::Unavailable)
    }
    fn sync_cloud(&self, pair: &Pair) -> Result<Arc<paranoic_cloud::CloudStorage>> {
        self.clouds.lock().map_err(lock_error)?.get(&pair.baseline.remote().storage).cloned().ok_or(BridgeError::Unavailable)
    }
    fn sync_options(pair: &Pair) -> SyncOptions { SyncOptions { mode: pair.mode, trash_remote_deletions: pair.trash_remote_deletions } }
    /// Decisions must not leave a stale executable plan behind, or discard IDs
    /// of jobs already accepted by Core. Finish that saved plan first.
    fn sync_before_decision(&self, pair: &mut Pair) -> Result<()> {
        if pair.plan.as_ref().is_some_and(|p| p.actions.iter()
            .any(|a| a.job.is_some_and(|job| self.core.job(job).is_ok()))) {
            return Err(BridgeError::Busy);
        }
        pair.plan = None;
        Ok(())
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Saved sync pairs (needs the device record key).
    pub fn sync_pairs(&self) -> Result<Vec<NativeSyncPair>> {
        let _guard = self.sync_guard()?;
        Ok(self.sync_load()?.pairs.iter().map(native).collect())
    }
    /// Pair a local folder with a folder of a connected cloud. `mode`: 1 backup,
    /// 2 two-way. `trash_remote_deletions`: files deleted locally go to the
    /// cloud trash (only where the server confirmed one); otherwise reported.
    pub fn sync_pair_add(&self, local: NativeLocation, remote: NativeLocation, mode: u8, trash_remote_deletions: bool) -> Result<NativeSyncPair> {
        let _guard = self.sync_guard()?;
        let (local, remote) = (local.core()?, remote.core()?);
        let mode = match mode { 1 => SyncMode::Backup, 2 => SyncMode::TwoWay, _ => return Err(BridgeError::Invalid) };
        if local.storage == remote.storage { return Err(BridgeError::Invalid); }
        // The app's own workspace holds spools with decrypted data (export,
        // archive extraction): never a sync side (external audit A-1).
        if local.storage == self.app.id() || remote.storage == self.app.id() { return Err(BridgeError::Denied); }
        let clouds = self.clouds.lock().map_err(lock_error)?;
        if !clouds.contains_key(&remote.storage) || clouds.contains_key(&local.storage) { return Err(BridgeError::Invalid); }
        drop(clouds);
        for location in [&local, &remote] {
            if self.core.metadata(location)?.kind != paranoic_core::EntryKind::Directory { return Err(BridgeError::Invalid); }
        }
        let mut state = self.sync_load()?;
        if state.pairs.len() >= MAX_PAIRS { return Err(BridgeError::Limit); }
        if state.pairs.iter().any(|p| p.baseline.local() == &local && p.baseline.remote() == &remote) { return Err(BridgeError::Conflict); }
        let pair = Pair { id: Uuid::new_v4().to_string(), mode, trash_remote_deletions, baseline: SyncBaseline::new(local, remote), plan: None };
        let result = native(&pair);
        state.pairs.push(pair);
        self.sync_store(&state)?;
        Ok(result)
    }
    /// Forget a pair (no file is touched).
    pub fn sync_pair_remove(&self, id: String) -> Result<()> {
        let _guard = self.sync_guard()?;
        let mut state = self.sync_load()?;
        let before = state.pairs.len();
        state.pairs.retain(|p| p.id != id);
        if state.pairs.len() == before { return Err(BridgeError::Invalid); }
        self.sync_store(&state)
    }
    /// Compare both folders and plan the changes (nothing is changed). The plan
    /// is saved; `sync_run` carries it out. Blocking: lists both trees.
    pub fn sync_plan(&self, id: String) -> Result<NativeSyncPlan> {
        let _guard = self.sync_guard()?;
        let mut state = self.sync_load()?;
        let pair = state.pairs.iter_mut().find(|p| p.id == id).ok_or(BridgeError::Invalid)?;
        // A plan whose jobs may already be in the journal is finished first.
        if pair.plan.as_ref().is_some_and(|p| p.actions.iter().any(|a| a.job.is_some_and(|job| self.core.job(job).is_ok()))) {
            return Err(BridgeError::Busy);
        }
        let cloud = self.sync_cloud(pair)?;
        let plan = pair.baseline.plan(&self.core, &cloud, Self::sync_options(pair), &std::sync::atomic::AtomicBool::new(false))?;
        let result = summary(&plan);
        pair.plan = Some(plan);
        self.sync_store(&state)?;
        Ok(result)
    }
    /// Carry out the saved plan: every change is an ordinary Core job (shown in
    /// the jobs list, cancellable); the baseline advances only for what really
    /// synchronized. Run again after an interruption: no job is repeated.
    /// Blocking, possibly long: call off the UI thread.
    pub fn sync_run(&self, id: String) -> Result<NativeSyncResult> {
        let _guard = self.sync_guard()?;
        let mut state = self.sync_load()?;
        let index = state.pairs.iter().position(|p| p.id == id).ok_or(BridgeError::Invalid)?;
        let cloud = self.sync_cloud(&state.pairs[index])?;
        let mut plan = state.pairs[index].plan.clone().ok_or(BridgeError::Invalid)?;
        let submitted = state.pairs[index].baseline.submit(&self.core, &cloud, &mut plan);
        // Persist what was submitted before waiting: a crash cannot lose job IDs.
        state.pairs[index].plan = Some(plan.clone());
        self.sync_store(&state)?;
        submitted?;
        let (mut done, mut failed) = (0u32, 0u32);
        for job in plan.actions.iter().filter_map(|a| a.job) {
            if self.core.job(job).is_err() { continue; } // not submitted (changed before submit)
            match features::run_wait(&self.core, job)?.first().map(|r| r.state) {
                Some(paranoic_core::JobState::Succeeded) => done += 1,
                _ => failed += 1,
            }
        }
        let pair = &mut state.pairs[index];
        pair.baseline.commit(&self.core, &cloud, &plan)?;
        pair.plan = None;
        self.sync_store(&state)?;
        Ok(NativeSyncResult { done, failed, reports: plan.reports.iter().map(report).collect() })
    }
    /// The user's decision for a conflicting path: keep the local version
    /// (`keep_local`) or the cloud one. The next plan replaces the other side.
    pub fn sync_resolve(&self, id: String, path: String, keep_local: bool) -> Result<()> {
        let _guard = self.sync_guard()?;
        let mut state = self.sync_load()?;
        let pair = state.pairs.iter_mut().find(|p| p.id == id).ok_or(BridgeError::Invalid)?;
        self.sync_before_decision(pair)?;
        pair.baseline.resolve_conflict(&self.core, &path, if keep_local { ConflictChoice::KeepLocal } else { ConflictChoice::KeepRemote })?;
        self.sync_store(&state)
    }
    /// Accept a reported deletion (or other report) for `path` and below: the
    /// baseline forgets it, nothing is deleted on either side.
    pub fn sync_forget(&self, id: String, path: String) -> Result<()> {
        let _guard = self.sync_guard()?;
        let mut state = self.sync_load()?;
        let pair = state.pairs.iter_mut().find(|p| p.id == id).ok_or(BridgeError::Invalid)?;
        self.sync_before_decision(pair)?;
        pair.baseline.forget(&path);
        self.sync_store(&state)
    }
}
