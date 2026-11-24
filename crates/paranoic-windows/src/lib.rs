//! Windows platform calls for Paranoic. The ONLY crate in the workspace where
//! `unsafe` is allowed (owner decision 2026-11-12). Rules: every unsafe block is
//! one Win32 call with a `SAFETY:` comment; every OS resource is released by an
//! RAII guard; nothing here decides policy — callers do.
//!
//! * `ensure_owner_only` / `is_owner_only`: a directory accessible only to the
//!   current user (plus SYSTEM and Administrators, as in %LOCALAPPDATA%), with
//!   a protected DACL so nothing is inherited from the parent.
//! * `file_id`: 128-bit file identity (volume serial + FILE_ID_128), stable on
//!   NTFS and ReFS, used to bind a folder grant to the same folder.
//! * `confine_current_process`: job object (kill on close, one process, memory
//!   limit, no UI/clipboard) and Low integrity level — for the PDF worker.
//!
//! On other platforms every function returns `Unsupported`.

use std::io;

#[cfg(windows)]
mod imp;
#[cfg(windows)]
mod worker;
#[cfg(windows)]
pub use worker::{spawn_document_worker, document_worker_is_confined, SandboxedWorker};

/// Directory ACL check: the owner is the current user (or SYSTEM /
/// Administrators, the default owner under an elevated token) and the DACL allows
/// access only to the current user, SYSTEM and Administrators (inherited or
/// explicit). A missing (NULL) DACL or any other allow entry fails.
pub fn is_owner_only(path: &std::path::Path) -> io::Result<bool> {
    #[cfg(windows)] { imp::is_owner_only(path) }
    #[cfg(not(windows))] { let _ = path; Err(unsupported()) }
}

/// Replace the DACL of `path` with: current user, SYSTEM, Administrators —
/// full control, inherited by children; protected (parent ACEs dropped); the
/// owner set to the current user.
pub fn ensure_owner_only(path: &std::path::Path) -> io::Result<()> {
    #[cfg(windows)] { imp::ensure_owner_only(path) }
    #[cfg(not(windows))] { let _ = path; Err(unsupported()) }
}

/// Volume serial number and 128-bit file id of an open file or directory.
#[cfg(windows)]
pub fn file_id(handle: std::os::windows::io::BorrowedHandle<'_>) -> io::Result<(u64, [u8; 16])> { imp::file_id(handle) }

/// Atomic rename that fails if `to` exists (MoveFileExW without
/// MOVEFILE_REPLACE_EXISTING; same volume only; write-through).
pub fn rename_no_replace(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    #[cfg(windows)] { imp::rename_no_replace(from, to) }
    #[cfg(not(windows))] { let _ = (from, to); Err(unsupported()) }
}

/// Limits for `confine_current_process`.
#[derive(Clone, Copy, Debug)]
pub struct Confinement { pub memory_bytes: usize }

/// Put the current process into a job (it cannot create processes, is killed
/// when its last job handle closes, has a memory cap, no clipboard/desktop
/// access) and lower its integrity level to Low (cannot write to files or
/// talk to processes of Medium integrity). Irreversible for the process.
pub fn confine_current_process(limits: Confinement) -> io::Result<()> {
    #[cfg(windows)] { imp::confine_current_process(limits) }
    #[cfg(not(windows))] { let _ = limits; Err(unsupported()) }
}

#[cfg(not(windows))]
fn unsupported() -> io::Error { io::Error::new(io::ErrorKind::Unsupported, "Windows only") }

