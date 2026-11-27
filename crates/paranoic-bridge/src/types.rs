use paranoic_core::{CoreError, EntryKind, ErrorCode, Location, ResourcePath, StorageId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("permission denied")] Denied,
    #[error("unsupported capability")] Unsupported,
    #[error("invalid request")] Invalid,
    #[error("conflict")] Conflict,
    #[error("resource limit")] Limit,
    #[error("source changed")] Changed,
    #[error("requires attention")] Attention,
    #[error("operation unavailable")] Unavailable,
    #[error("in use")] Busy,
    #[error("cancelled")] Cancelled,
    #[error("wrong password")] WrongPassword,
    /// The content is encrypted and no password was given (archives, E2.1).
    #[error("password required")] PasswordRequired,
    #[error("password too weak")] WeakPassword,
    #[error("password attempts limited")] RateLimited { retry_after_ms: u64 },
    #[error("erased after too many wrong passwords")] Wiped,
    #[error("not set up")] NotSetUp,
    #[error("locked")] Locked,
    #[error("device key unavailable")] DeviceKey,
}
impl From<CoreError> for BridgeError {
    fn from(e: CoreError) -> Self {
        crate::diagnostics::record(&e);
        // Archive password outcomes travel as fixed messages on PermissionDenied.
        if paranoic_documents::is_password_required(&e) { return Self::PasswordRequired; }
        if paranoic_documents::is_wrong_password(&e) { return Self::WrongPassword; }
        match e.code {
        ErrorCode::PermissionDenied => Self::Denied,
        ErrorCode::Unsupported => Self::Unsupported,
        ErrorCode::InvalidInput | ErrorCode::InvalidPath => Self::Invalid,
        ErrorCode::AlreadyExists => Self::Conflict,
        ErrorCode::LimitExceeded => Self::Limit,
        ErrorCode::SourceChanged | ErrorCode::Integrity => Self::Changed,
        ErrorCode::NeedsAttention => Self::Attention,
        ErrorCode::Busy => Self::Busy,
        ErrorCode::Cancelled => Self::Cancelled,
        _ => Self::Unavailable,
    }
    }
}
pub type Result<T> = std::result::Result<T, BridgeError>;
#[derive(Clone, uniffi::Record)]
pub struct NativeLocation { pub storage_id: String, pub path: String }
impl NativeLocation {
    pub(crate) fn core(&self) -> Result<Location> {
        Ok(Location { storage: StorageId(Uuid::parse_str(&self.storage_id).map_err(|_| BridgeError::Invalid)?),
            path: ResourcePath::parse(self.path.clone())? })
    }
}
impl From<Location> for NativeLocation {
    fn from(l: Location) -> Self { Self { storage_id: l.storage.0.to_string(), path: l.path.as_str().to_owned() } }
}
#[derive(Clone, uniffi::Record)]
pub struct NativeGrant { pub id: String, pub reference: String }
#[derive(Clone, uniffi::Record)]
/// `detachable`: a user-connected folder (false for the app's own space).
pub struct NativeSpace { pub id: String, pub name: String, pub root: NativeLocation, pub writable: bool, pub detachable: bool }
#[derive(Clone, uniffi::Record)]
pub struct NativeListing { pub entries: Vec<NativeEntry>, pub skipped: u64 }
#[derive(Clone, uniffi::Record)]
pub struct NativeEntry { pub location: NativeLocation, pub name: String, pub kind: u8, pub bytes: u64, pub modified_ms: Option<u64> }
pub(crate) fn kind(k: EntryKind) -> u8 { match k { EntryKind::File => 1, EntryKind::Directory => 2, EntryKind::Symlink => 3, _ => 4 } }
/// Journal snapshot for display and recovery. `kind`: 1 create folder,
/// 2 create file, 3 copy, 4 move, 5 delete, 6 save/publish, 7 recovery,
/// 8 rename. `error`: stable category (see `error_code`), never OS text.
/// `target` is the requested destination. After success, `outcome` is 1 done
/// or 2 skipped (name taken, nothing written); 0 while not succeeded.
/// `result_target` is where the result actually is when Core chose another
/// name ("keep both": `name (N).ext`), otherwise None.
#[derive(Clone, uniffi::Record)]
pub struct NativeJob {
    pub id: String, pub phase: u8, pub bytes_done: u64, pub bytes_total: Option<u64>, pub can_cancel: bool,
    pub kind: u8, pub source: Option<NativeLocation>, pub target: Option<NativeLocation>, pub error: Option<u8>,
    pub outcome: u8, pub result_target: Option<NativeLocation>,
}
pub(crate) fn error_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::PermissionDenied => 1, ErrorCode::AlreadyExists => 2, ErrorCode::NotFound => 3,
        ErrorCode::SourceChanged | ErrorCode::Integrity => 4, ErrorCode::LimitExceeded => 5,
        ErrorCode::Unsupported => 6, ErrorCode::Cancelled => 7, ErrorCode::NeedsAttention => 8,
        ErrorCode::InvalidInput | ErrorCode::InvalidPath => 9, ErrorCode::Io => 10, _ => 11,
    }
}
fn job_kind(operation: &paranoic_core::Operation) -> (u8, Option<&Location>, &Location) {
    use paranoic_core::Operation::*;
    match operation {
        // Same folder, new name: shown as a rename.
        Move { source, target } if source.storage == target.storage
            && source.path.parent_and_name().ok().map(|p| p.0) == target.path.parent_and_name().ok().map(|p| p.0) => (8, Some(source), target),
        CreateDirectory { target } => (1, None, target),
        CreateFile { target } => (2, None, target),
        CopyFile { source, target, .. } | CopyDirectory { source, target, .. } | CopyVerified { source, target }
        | CopyFileExpected { source, target, .. } | CopyExpected { source, target, .. }
        | CopyExpectedBundle { source, target, .. } => (3, Some(source), target),
        MoveFileAcross { source, target } | MoveDirectoryAcross { source, target }
        | MoveDirectoryExpected { source, target, .. } | Move { source, target } => (4, Some(source), target),
        DeleteFile { target } | DeleteFileExpected { target, .. } | DeleteFileContent { target, .. } | DeleteEmptyDirectory { target }
        | DeleteDirectory { target } => (5, None, target),
        ReplaceExpected { source, target, .. } | ReplaceContentExpected { source, target, .. } => (6, Some(source), target),
        PublishTree { target, .. } | PublishLinkedTrees { target, .. } => (6, None, target),
        RecoverMapped { target, .. } | RecoverReshape { target, .. } => (7, None, target),
        RenameMapped { source, target, .. } | ReshapeDirectory { source, target, .. } => (8, Some(source), target),
    }
}
impl From<paranoic_core::JobRecord> for NativeJob {
    fn from(j: paranoic_core::JobRecord) -> Self {
        use paranoic_core::JobState::*;
        let phase = match j.state { Queued => 1, Running => 2, Succeeded => 3, Failed => 4, Cancelled => 5, NeedsAttention => 6 };
        let (kind, source, target) = job_kind(&j.operation);
        Self { id: j.id.0.to_string(), phase, bytes_done: j.bytes_done, bytes_total: j.bytes_total,
            can_cancel: matches!(j.state, Queued | Running) && !j.cancel_requested,
            kind, source: source.cloned().map(Into::into), target: Some(target.clone().into()),
            error: j.error.map(|e| error_code(e.code)),
            outcome: match j.result.as_ref().map(|r| r.outcome) {
                Some(paranoic_core::Outcome::Applied) => 1, Some(paranoic_core::Outcome::Skipped) => 2, None => 0,
            },
            result_target: j.result.and_then(|r| r.target).map(Into::into) }
    }
}
#[derive(Clone, uniffi::Record)]
pub struct HostBytes { pub error: u32, pub data: Vec<u8> }
#[derive(Clone, uniffi::Record)]
pub struct NativePreview { pub kind: u8, pub mime: String, pub length: u64 }
#[derive(Clone, uniffi::Record)]
pub struct NativePdfPage { pub pages: u32, pub index: u32, pub png: Vec<u8> }
#[derive(Clone, uniffi::Record)]
pub struct NativePdfMatches { pub pages: Vec<u32>, pub next_page: Option<u32> }

