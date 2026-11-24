//! Plan 1.3: jobs that need attention — Core could neither finish nor cleanly
//! undo them, so the user decides. Nothing here resolves anything by itself:
//! every action is an explicit call, file actions are journaled Core jobs, and
//! a backup copy is never deleted while it is the only version of the file.
use crate::*;
use paranoic_core::{JobRecord, JobState, Operation};

/// Why the job needs attention (UI chooses the localized explanation).
/// 1 interrupted by a crash/stop while writing; 2 file replacement left the old
/// version in a hidden backup; 3 folder copy/move/delete or cross-storage
/// transfer stopped part-way; 4 prepared publication (archive extraction,
/// encrypted folder) interrupted; 5 Cryptomator rename/reorganisation
/// interrupted; 0 other.
#[derive(Clone, uniffi::Record)]
pub struct NativeAttention {
    pub job: NativeJob,
    pub reason: u8,
    /// Core's technical message (English); for details only, not the main text.
    pub detail: String,
    /// Hidden backup of a replaced file, when one exists.
    pub backup: Option<NativeLocation>,
    /// Backup present and the target name free: the old version can go back.
    pub can_restore: bool,
    /// Backup present and the new file in place: the backup can be deleted.
    pub can_discard: bool,
    /// Prepared publication whose result can be checked and accepted.
    pub can_accept: bool,
}

const LIST_LIMIT: usize = 1000;

fn reason(record: &JobRecord) -> u8 {
    let interrupted = record.error.as_ref().is_some_and(|e| e.message.starts_with("interrupted operation"));
    if backup_of(record).is_some() { return 2; }
    match &record.operation {
        Operation::PublishTree { .. } | Operation::PublishLinkedTrees { .. } | Operation::CopyExpected { .. }
        | Operation::CopyExpectedBundle { .. } | Operation::ReplaceExpected { .. } | Operation::ReplaceContentExpected { .. }
        | Operation::MoveDirectoryExpected { .. } => 4,
        Operation::RenameMapped { .. } | Operation::ReshapeDirectory { .. } | Operation::RecoverMapped { .. }
        | Operation::RecoverReshape { .. } => 5,
        _ if interrupted => 1,
        Operation::CopyDirectory { .. } | Operation::MoveDirectoryAcross { .. } | Operation::DeleteDirectory { .. }
        | Operation::MoveFileAcross { .. } | Operation::CopyVerified { .. } => 3,
        _ => 0,
    }
}

/// The hidden sibling Core moved the old file to during a Replace copy.
fn backup_of(record: &JobRecord) -> Option<Location> {
    let target = match &record.operation {
        Operation::CopyFile { target, conflict: paranoic_core::ConflictPolicy::Replace, .. }
        | Operation::ReplaceExpected { target, .. } | Operation::ReplaceContentExpected { target, .. } => target,
        _ => return None,
    };
    let entry = record.active_entry.as_ref()?;
    let (parent, name) = entry.path.parent_and_name().ok()?;
    let (target_parent, _) = target.path.parent_and_name().ok()?;
    (entry.storage == target.storage && parent == target_parent && name.starts_with(".pnc-replaced-")).then(|| entry.clone())
}

impl NativeRuntime {
    fn attention_record(&self, id: &str) -> Result<JobRecord> {
        let record = self.core.job(JobId(grant_id(id)?.0))?;
        if record.state != JobState::NeedsAttention || record.reviewed_unix_ms.is_some() { return Err(BridgeError::Invalid); }
        Ok(record)
    }
    fn exists(&self, location: &Location) -> Result<bool> {
        match self.core.metadata(location) {
            Ok(_) => Ok(true),
            Err(e) if e.code == paranoic_core::ErrorCode::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    fn describe(&self, record: JobRecord) -> Result<NativeAttention> {
        let backup = backup_of(&record);
        let (mut can_restore, mut can_discard) = (false, false);
        if let Some(backup) = &backup {
            if self.exists(backup)? {
                let target_present = self.exists(record.operation.target())?;
                can_restore = !target_present;
                can_discard = target_present;
            }
        }
        let can_accept = matches!(record.operation, Operation::PublishTree { .. } | Operation::PublishLinkedTrees { .. })
            && record.effects_started && record.bootstrap_acceptance.is_none() && record.linked_trees_acceptance.is_none();
        Ok(NativeAttention { reason: reason(&record), detail: record.error.as_ref().map(|e| e.message.clone()).unwrap_or_default(),
            backup: backup.map(Into::into), can_restore, can_discard, can_accept, job: record.into() })
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Open NeedsAttention jobs (not yet reviewed or accepted), oldest first.
    pub fn attention_list(&self) -> Result<Vec<NativeAttention>> {
        let mut out = Vec::new();
        let mut after = 0;
        loop {
            let page = self.core.unacknowledged_attention(after, 200)?;
            for record in page.jobs { out.push(self.describe(record)?); }
            match page.next_cursor { Some(next) if out.len() < LIST_LIMIT => after = next, _ => return Ok(out) }
        }
    }
    /// The user looked at both locations and closes the job. Changes no files.
    pub fn attention_reviewed(&self, id: String) -> Result<()> {
        self.attention_record(&id)?;
        self.core.mark_reviewed(JobId(grant_id(&id)?.0))?;
        Ok(())
    }
    /// Put the previous version back under the original name (a Core move,
    /// never replacing). Only while that name is free. After the returned job
    /// succeeds, close the attention job with `attention_reviewed`.
    pub fn attention_restore(&self, id: String, request_id: String) -> Result<NativeJob> {
        let record = self.attention_record(&id)?;
        let Some(backup) = backup_of(&record) else { return Err(BridgeError::Unsupported) };
        let target = record.operation.target();
        if !self.exists(&backup)? { return Err(BridgeError::Changed); }
        if self.exists(target)? { return Err(BridgeError::Conflict); }
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), Operation::Move { source: backup, target: target.clone() })?.into())
    }
    /// Delete the hidden backup — only when the new file is in place, and only
    /// the exact backup seen now (a changed backup is not deleted).
    pub fn attention_discard(&self, id: String, request_id: String) -> Result<NativeJob> {
        let record = self.attention_record(&id)?;
        let Some(backup) = backup_of(&record) else { return Err(BridgeError::Unsupported) };
        let target = record.operation.target();
        if !self.exists(target)? { return Err(BridgeError::Denied); }
        let expected = self.core.metadata(&backup)?;
        Ok(self.core.submit_with_id(JobId(grant_id(&request_id)?.0), Operation::DeleteFileExpected { target: backup, expected, expected_revision: None })?.into())
    }
    /// Re-read a prepared publication and accept it only if it matches exactly
    /// what was prepared (Integrity otherwise). Blocking: call off the UI thread.
    pub fn attention_accept(&self, id: String) -> Result<()> {
        let record = self.attention_record(&id)?;
        let never = std::sync::atomic::AtomicBool::new(false);
        match record.operation {
            Operation::PublishTree { .. } => { self.core.accept_prepared_tree(record.id, &never)?; }
            Operation::PublishLinkedTrees { .. } => { self.core.accept_linked_trees(record.id, &never)?; }
            _ => return Err(BridgeError::Unsupported),
        }
        Ok(())
    }
}
