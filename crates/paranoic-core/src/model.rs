use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CoreError, ErrorCode, ResourcePath, Result};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub Uuid);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub storage: StorageId,
    pub path: ResourcePath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind { File, Directory, Symlink, Other }

/// Metadata is descriptive, never an authorization or identity token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub kind: EntryKind,
    pub bytes: u64,
    pub modified_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub location: Location,
    pub metadata: Metadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Fail,
    Skip,
    /// Choose the first free "name (N).ext" (N = 1..=9999). The chosen
    /// location is journaled as active_entry before any write and returned
    /// in OperationResult::target.
    KeepBoth,
    /// File over file only. The existing file is moved to a hidden sibling,
    /// the new file is published, then the old one is deleted. If publication
    /// fails the old file is moved back. Directories are never replaced.
    Replace,
}

/// Expected content of one file, relative to the moved directory root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedTreeFile {
    pub path: ResourcePath,
    pub bytes: u64,
    pub blake3: [u8; 32],
}

/// Small public/encrypted bootstrap files persisted verbatim in the journal.
/// Never put passwords, unwrapped keys or plaintext user content here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedTreeFile {
    pub path: ResourcePath,
    pub bytes: Vec<u8>,
}

/// Bounded bootstrap layout; directories are implied by file paths.
pub fn validate_prepared_tree(files: &[PreparedTreeFile]) -> Result<u64> {
    if files.is_empty() || files.len() > 32 {
        return Err(CoreError::new(ErrorCode::LimitExceeded, "bootstrap file count out of bounds"));
    }
    let mut total = 0_u64;
    for (index, file) in files.iter().enumerate() {
        if file.path.is_root() || file.path.as_str().split('/').count() > 8 {
            return Err(CoreError::new(ErrorCode::InvalidPath, "invalid bootstrap path"));
        }
        total = total.checked_add(file.bytes.len() as u64).ok_or_else(CoreError::internal)?;
        if total > 65536 {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "bootstrap exceeds byte budget"));
        }
        for other in &files[..index] {
            let a = file.path.as_str();
            let b = other.path.as_str();
            if a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/")) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "bootstrap paths overlap"));
            }
        }
    }
    Ok(total)
}

/// Mutations never replace an existing destination. Recursive deletion and
/// cross-provider moves deliberately require a separate, future contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    PublishTree { target: Location, files: Vec<PreparedTreeFile> },
    /// Ordered create-new publication at two disjoint locations on one grant.
    /// The second tree is exposed only after readback of the first. Not atomic
    /// across both locations; partial publication is retained for recovery.
    PublishLinkedTrees { data: Location, data_files: Vec<PreparedTreeFile>,
        target: Location, files: Vec<PreparedTreeFile> },
    CreateDirectory { target: Location },
    CreateFile { target: Location },
    CopyFile { source: Location, target: Location, conflict: ConflictPolicy },
    CopyDirectory { source: Location, target: Location, conflict: ConflictPolicy },
    CopyVerified { source: Location, target: Location },
    /// Replace an existing file ("Save") only if it still has exactly the
    /// metadata observed when it was opened, and only with the prepared bytes.
    /// The previous version goes through the regular journaled Replace path.
    ReplaceExpected { source: Location, target: Location, bytes: u64, blake3: [u8; 32], expected_target: Metadata },
    /// Text/editor Save additionally binds the original encoded contents.
    /// Keep ReplaceExpected unchanged for existing callers and journal records.
    ReplaceContentExpected { source: Location, target: Location, bytes: u64, blake3: [u8; 32],
        expected_target: Metadata, expected_target_blake3: [u8; 32] },
    /// Copy only if the source still has `expected_source`; the destination
    /// must be absent (`expected_target` None; with `keep_both` a free
    /// "name (N)" is chosen instead) or exactly `expected_target`, which is
    /// then replaced. Checked when the job runs, not when it was planned.
    CopyFileExpected { source: Location, target: Location, expected_source: Metadata,
        expected_target: Option<Metadata>, keep_both: bool,
        /// Provider revisions (e.g. WebDAV ETag) seen when the copy was planned.
        /// Size and time alone miss a same-size edit within the clock
        /// resolution (У-24). Checked against `Storage::provider_revision`;
        /// absent in older journals and omitted when not set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_source_revision: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_target_revision: Option<String> },
    /// Delete exactly the object whose content was verified (У-69): the
    /// storage pins the object (`prepare_move_source`), Core checks its
    /// metadata and BLAKE3 through that pin, and only the pinned object is
    /// removed. Storages that cannot pin refuse (Unsupported), nothing deleted.
    DeleteFileContent { target: Location, expected: Metadata, expected_blake3: [u8; 32] },
    /// Delete a file only if it still has `expected`.
    DeleteFileExpected { target: Location, expected: Metadata,
        /// Provider revision (e.g. WebDAV ETag) seen when the deletion was
        /// planned: only that version is deleted (conditional request), a
        /// newer one written meanwhile is kept (external audit A-4).
        /// Absent in older journals and omitted when not set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String> },
    /// Copy only the exact prepared bytes; expectation is persisted in the journal.
    CopyExpected { source: Location, target: Location, bytes: u64, blake3: [u8; 32] },
    CopyExpectedBundle { source: Location, target: Location, bytes: u64, blake3: [u8; 32],
        content_name: String, sidecar_name: String, sidecar: Vec<u8> },
    MoveFileAcross { source: Location, target: Location },
    /// Copy a tree to another storage, read back every file, then remove the
    /// source entries that are unchanged. Not atomic; failures after the copy
    /// leave both trees for inspection (NeedsAttention).
    MoveDirectoryAcross { source: Location, target: Location },
    /// MoveDirectoryAcross bound to a prepared manifest (e.g. archive
    /// extraction): after the copy, the set of files, their lengths and
    /// BLAKE3 must equal `files` exactly BEFORE any source is removed.
    MoveDirectoryExpected { source: Location, target: Location, files: Vec<ExpectedTreeFile> },
    Move { source: Location, target: Location },
    ReshapeDirectory { source: Location, target: Location, layout: DirectoryReshape },
    RecoverMapped { original: JobId, target: Location, action: MappedRecoveryAction },
    /// Explicit repair of a retained ReshapeDirectory wrapper (see RecoverMapped).
    RecoverReshape { original: JobId, target: Location, action: MappedRecoveryAction },
    RenameMapped { source: Location, target: Location, old_mapping: Vec<u8>, new_mapping: Vec<u8> },
    DeleteFile { target: Location },
    DeleteEmptyDirectory { target: Location },
    DeleteDirectory { target: Location },
}

impl Operation {
    /// Every location the operation reads or writes (used to keep a storage
    /// registered while unfinished jobs still refer to it).
    pub fn locations(&self) -> Vec<&Location> {
        use Operation::*;
        match self {
            PublishTree { target, .. } | CreateDirectory { target } | CreateFile { target }
            | DeleteFileExpected { target, .. } | DeleteFileContent { target, .. } | RecoverMapped { target, .. } | RecoverReshape { target, .. }
            | DeleteFile { target } | DeleteEmptyDirectory { target } | DeleteDirectory { target } => vec![target],
            PublishLinkedTrees { data, target, .. } => vec![data, target],
            CopyFile { source, target, .. } | CopyDirectory { source, target, .. } | CopyVerified { source, target }
            | ReplaceExpected { source, target, .. } | ReplaceContentExpected { source, target, .. } | CopyFileExpected { source, target, .. }
            | CopyExpected { source, target, .. } | CopyExpectedBundle { source, target, .. }
            | MoveFileAcross { source, target } | MoveDirectoryAcross { source, target }
            | MoveDirectoryExpected { source, target, .. } | Move { source, target }
            | ReshapeDirectory { source, target, .. } | RenameMapped { source, target, .. } => vec![source, target],
        }
    }
    pub fn validate(&self) -> Result<()> {
        if let Self::PublishTree { target, files } = self {
            validate_prepared_tree(files)?;
            for file in files {
                ResourcePath::parse(format!("{}/{}", target.path.as_str(), file.path.as_str()))?;
            }
        }
        if let Self::PublishLinkedTrees { data, data_files, target, files } = self {
            if data.storage != target.storage || data.path.is_root() || data == target
                || data.path.as_str().starts_with(&format!("{}/", target.path.as_str()))
                || target.path.as_str().starts_with(&format!("{}/", data.path.as_str())) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "linked publication locations overlap or differ in storage"));
            }
            for (location, contents) in [(data, data_files), (target, files)] {
                validate_prepared_tree(contents)?;
                for file in contents {
                    ResourcePath::parse(format!("{}/{}", location.path.as_str(), file.path.as_str()))?;
                }
            }
        }
        if let Self::CopyExpectedBundle { content_name, sidecar_name, sidecar, .. } = self {
            ResourcePath::root().join(content_name)?;
            ResourcePath::root().join(sidecar_name)?;
            if content_name == sidecar_name || sidecar.len() > 4096 {
                return Err(CoreError::new(ErrorCode::InvalidInput, "invalid bundle layout"));
            }
        }
        if self.target().path.is_root() {
            return Err(CoreError::new(ErrorCode::InvalidPath, "storage roots cannot be mutated"));
        }
        // KeepBoth may copy an entry onto its own name: the executor chooses a
        // free sibling name ("name (1)") before any write.
        let duplicate = matches!(self, Self::CopyFile { conflict: ConflictPolicy::KeepBoth, .. }
            | Self::CopyDirectory { conflict: ConflictPolicy::KeepBoth, .. });
        if let Some(source) = self.source() {
            if source.path.is_root() || (source == self.target() && !duplicate) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "invalid operation source"));
            }
        }
        if let Self::ReshapeDirectory { source, target, layout } = self {
            layout.validate()?;
            if source.storage != target.storage || target.path.as_str().starts_with(&format!("{}/", source.path.as_str())) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "invalid directory reshape locations"));
            }
        }
        if let Self::RenameMapped { source, target, old_mapping, new_mapping } = self {
            if source.storage != target.storage || target.path.as_str().starts_with(&format!("{}/", source.path.as_str()))
                || old_mapping.is_empty() || new_mapping.is_empty() || old_mapping.len() > 368 || new_mapping.len() > 368 {
                return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped rename"));
            }
        }
        if let Self::Move { source, target } = self {
            if source.storage != target.storage {
                return Err(CoreError::new(ErrorCode::Unsupported, "cross-storage move is not supported"));
            }
            let prefix = format!("{}/", source.path.as_str());
            if target.path.as_str().starts_with(&prefix) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "cannot move an entry into itself"));
            }
        }
        if let Self::MoveDirectoryAcross { source, target } | Self::MoveDirectoryExpected { source, target, .. } = self {
            if source.storage == target.storage {
                return Err(CoreError::new(ErrorCode::InvalidInput, "use Move within one storage"));
            }
        }
        if let Self::MoveDirectoryExpected { files, .. } = self {
            if files.len() > 100_000 {
                return Err(CoreError::new(ErrorCode::LimitExceeded, "manifest too large"));
            }
            let mut seen = std::collections::HashSet::new();
            if files.iter().any(|f| f.path.is_root() || !seen.insert(f.path.as_str())) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "invalid manifest path"));
            }
        }
        if let Self::CopyDirectory { source, target, .. } = self {
            if source.storage == target.storage && target.path.as_str().starts_with(&format!("{}/", source.path.as_str())) {
                return Err(CoreError::new(ErrorCode::InvalidInput, "cannot copy a directory into itself"));
            }
        }
        Ok(())
    }

    pub fn target(&self) -> &Location {
        match self {
            Self::PublishLinkedTrees { target, .. } | Self::PublishTree { target, .. } | Self::CreateDirectory { target } | Self::CreateFile { target }
            | Self::ReshapeDirectory { target, .. } | Self::RecoverMapped { target, .. } | Self::RecoverReshape { target, .. } | Self::RenameMapped { target, .. } | Self::CopyFile { target, .. } | Self::Move { target, .. }
            | Self::CopyDirectory { target, .. } | Self::ReplaceContentExpected { target, .. }
            | Self::CopyExpectedBundle { target, .. } | Self::CopyExpected { target, .. } | Self::CopyVerified { target, .. } | Self::ReplaceExpected { target, .. } | Self::CopyFileExpected { target, .. } | Self::MoveFileAcross { target, .. }
            | Self::MoveDirectoryAcross { target, .. } | Self::MoveDirectoryExpected { target, .. }
            | Self::DeleteFile { target } | Self::DeleteFileExpected { target, .. } | Self::DeleteFileContent { target, .. } | Self::DeleteEmptyDirectory { target }
            | Self::DeleteDirectory { target } => target,
        }
    }

    pub fn source(&self) -> Option<&Location> {
        match self {
            Self::ReshapeDirectory { source, .. } | Self::RenameMapped { source, .. } | Self::CopyFile { source, .. } | Self::Move { source, .. } => Some(source),
            Self::CopyDirectory { source, .. } | Self::ReplaceContentExpected { source, .. } => Some(source),
            Self::CopyExpectedBundle { source, .. } | Self::CopyExpected { source, .. } | Self::CopyVerified { source, .. } | Self::ReplaceExpected { source, .. } | Self::CopyFileExpected { source, .. } | Self::MoveFileAcross { source, .. }
            | Self::MoveDirectoryAcross { source, .. } | Self::MoveDirectoryExpected { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState { Queued, Running, Succeeded, Failed, Cancelled, NeedsAttention }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Applied, Skipped }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase { Copying, Publishing, Verifying, Verified, RemovingSource, SourceRemoved }

/// Evidence is historical: it does not authorize replay or prove a destination
/// still exists unchanged after the recorded read-back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferReceipt {
    pub phase: TransferPhase,
    pub bytes: u64,
    pub blake3: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationResult {
    pub outcome: Outcome,
    pub bytes: u64,
    /// BLAKE3 of bytes streamed to the staged destination; not a read-back proof.
    pub blake3: Option<String>,
    /// Actual destination when it differs from the request (ConflictPolicy::KeepBoth).
    #[serde(default)]
    pub target: Option<Location>,
    /// Provider revision of the destination read right after this job
    /// published it (e.g. WebDAV ETag), when the storage has revisions. Sync
    /// records this, not the revision found later at commit (audit A-5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub operation: Operation,
    pub state: JobState,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    #[serde(default)]
    pub entries_done: u64,
    #[serde(default)]
    pub entries_total: Option<u64>,
    /// Persisted before touching this entry; retained on uncertain completion.
    #[serde(default)]
    pub active_entry: Option<Location>,
    #[serde(default)]
    pub transfer: Option<TransferReceipt>,
    /// Explicit acceptance of freshly observed bootstrap bytes. The historical
    /// state/error remain intact; this is not a successful publication receipt.
    #[serde(default)]
    pub bootstrap_acceptance: Option<BootstrapAcceptance>,
    /// Separate decision for a two-location publication; never a partial-tree
    /// acceptance or permission to synthesize the absent half.
    #[serde(default)]
    pub linked_trees_acceptance: Option<LinkedTreesAcceptance>,
    /// The user reviewed this NeedsAttention job and closed it. Asserts nothing
    /// about storage contents (unlike the acceptances above); state and error stay.
    #[serde(default)]
    pub reviewed_unix_ms: Option<u64>,
    pub cancel_requested: bool,
    /// Persisted BEFORE the first potential side effect.
    pub effects_started: bool,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub result: Option<OperationResult>,
    pub error: Option<CoreError>,
}

/// Ascending durable sequence pagination. The cursor is independent of timestamps
/// and remains meaningful while other jobs are submitted or updated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobPage {
    pub jobs: Vec<JobRecord>,
    pub next_cursor: Option<u64>,
}

/// Point-in-time readback only. Never a permit to replay, delete or overwrite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedCopyObservation { Missing, Matches, Differs }

/// Observed discrepancy, not authorization to replace or delete an entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedTreeDifference { MissingEntry, EntryKind, Contents, DirectoryEntries, Changed }

/// Fresh evidence for a terminal PublishTree job. Matches describes current
/// visible bytes, not successful fsync, absence of staging artifacts or an
/// immutable snapshot. Paths remain relative to the job's storage grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreparedTreeObservation {
    Missing,
    Matches,
    Differs { path: ResourcePath, reason: PreparedTreeDifference },
}

/// A durable, immutable acknowledgement of a matching bootstrap observation.
/// It does not prove prior fsync completion or authorize staging cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAcceptance {
    pub recorded_unix_ms: u64,
    pub request_blake3: [u8; 32],
    pub storage_binding_blake3: [u8; 32],
    pub bytes: u64,
    pub files: u32,
}

/// Two sequential observations, not a transaction or a permit to replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedTreesObservation {
    pub data: PreparedTreeObservation,
    pub entry: PreparedTreeObservation,
}

/// Historical acceptance of both exact trees observed during one explicit
/// recovery request. Does not assert a cross-location snapshot or prior fsync.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedTreesAcceptance {
    pub recorded_unix_ms: u64,
    pub request_blake3: [u8; 32],
    pub storage_binding_blake3: [u8; 32],
    pub data_bytes: u64,
    pub entry_bytes: u64,
    pub data_files: u32,
    pub entry_files: u32,
}

/// Mapping-only observation, not authentication of file contents or ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappedEntryObservation { Missing, OldMapping, NewMapping, BothMappings, Different }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappedRenameObservation {
    pub source: MappedEntryObservation,
    pub target: MappedEntryObservation,
    pub staged_entry: MappedEntryObservation,
    pub staging_present: bool,
}

/// Explicit destination for a payload retained by an interrupted mapped move.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappedRecoveryAction { Complete, RestoreSource }

/// Exact public directory-link bytes and optional encrypted name mappings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryReshape {
    pub link: Vec<u8>,
    pub old_mapping: Option<Vec<u8>>,
    pub new_mapping: Option<Vec<u8>>,
}
impl DirectoryReshape {
    pub fn validate(&self) -> Result<()> {
        let text = std::str::from_utf8(&self.link).map_err(|_| CoreError::new(ErrorCode::InvalidInput, "invalid directory link"))?;
        let id = Uuid::parse_str(text).map_err(|_| CoreError::new(ErrorCode::InvalidInput, "invalid directory link"))?;
        if id.to_string() != text || self.old_mapping.is_some() == self.new_mapping.is_some()
            || [&self.old_mapping, &self.new_mapping].iter().any(|m| m.as_ref().is_some_and(|v| v.is_empty() || v.len() > 368)) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid directory reshape"));
        }
        Ok(())
    }
}
