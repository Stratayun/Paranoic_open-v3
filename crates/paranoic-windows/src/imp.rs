//! Win32 implementation. Each unsafe block: one call, documented contract.
use std::{ffi::c_void, io, os::windows::{ffi::OsStrExt, io::{AsRawHandle, BorrowedHandle}}, path::Path, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_ALL, HANDLE, HLOCAL},
    Security::{
        Authorization::{GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS, SE_FILE_OBJECT,
            TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W, NO_MULTIPLE_TRUSTEE},
        CreateWellKnownSid, EqualSid, GetAce, GetAclInformation, GetLengthSid, GetTokenInformation, IsValidSid, SetTokenInformation,
        TokenIntegrityLevel, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid, WinLowLabelSid, ACCESS_ALLOWED_ACE, ACE_HEADER,
        ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_ADJUST_DEFAULT, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, WELL_KNOWN_SID_TYPE,
    },
    Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, MoveFileExW, FILE_ID_INFO, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
    System::{
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
            JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
            JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const SE_GROUP_INTEGRITY: u32 = 0x20;

fn last_error() -> io::Error { io::Error::last_os_error() }
fn win32(code: u32) -> io::Result<()> { if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) } }
fn wide(path: &Path) -> Vec<u16> { path.as_os_str().encode_wide().chain(std::iter::once(0)).collect() }

/// Kernel handle closed on drop.
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a handle we own (returned by an Open*/Create* call) and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}
/// LocalAlloc'ed memory (security descriptors, ACLs) freed on drop.
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer was allocated by the system with LocalAlloc and is freed once.
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }
}

/// A SID in an owned, correctly sized buffer.
struct Sid(Vec<u8>);
impl Sid {
    fn ptr(&self) -> PSID { self.0.as_ptr() as PSID }
    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Self> {
        let mut buffer = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut size = buffer.len() as u32;
        // SAFETY: buffer has `size` writable bytes; domain SID may be null for these well-known types.
        if unsafe { CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr() as PSID, &mut size) } == 0 { return Err(last_error()); }
        buffer.truncate(size as usize);
        Ok(Self(buffer))
    }
    /// Copy of a SID referenced by `psid` (validated).
    fn copy(psid: PSID) -> io::Result<Self> {
        // SAFETY: IsValidSid only reads the structure behind a non-null pointer provided by the system.
        if psid.is_null() || unsafe { IsValidSid(psid) } == 0 { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid SID")); }
        // SAFETY: psid is a valid SID (checked above); GetLengthSid reads its header.
        let length = unsafe { GetLengthSid(psid) } as usize;
        // SAFETY: a valid SID occupies exactly `length` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(psid as *const u8, length) }.to_vec();
        Ok(Self(bytes))
    }
    fn equals(&self, other: PSID) -> bool {
        // SAFETY: both pointers reference valid SIDs (ours is owned; `other` comes from a validated ACE/descriptor).
        unsafe { EqualSid(self.ptr(), other) != 0 }
    }
}

fn process_token(access: u32) -> io::Result<Handle> {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; token receives a new handle owned by us.
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) } == 0 { return Err(last_error()); }
    Ok(Handle(token))
}

fn current_user() -> io::Result<Sid> {
    let token = process_token(TOKEN_QUERY)?;
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; fails with ERROR_INSUFFICIENT_BUFFER and sets `needed`.
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 || needed > 4096 { return Err(last_error()); }
    // u64 storage keeps TOKEN_USER suitably aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: buffer provides `needed` writable, aligned bytes.
    if unsafe { GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr() as *mut c_void, needed, &mut needed) } == 0 {
        return Err(last_error());
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER whose SID points inside the buffer.
    let user = unsafe { &*(buffer.as_ptr() as *const windows_sys::Win32::Security::TOKEN_USER) };
    Sid::copy(user.User.Sid)
}

pub(crate) fn is_owner_only(path: &Path) -> io::Result<bool> {
    let user = current_user()?;
    let allowed = [Sid::well_known(WinLocalSystemSid)?, Sid::well_known(WinBuiltinAdministratorsSid)?];
    let name = wide(path);
    let (mut owner, mut dacl, mut descriptor): (PSID, *mut ACL, PSECURITY_DESCRIPTOR) = (null_mut(), null_mut(), null_mut());
    // SAFETY: name is NUL-terminated; out-pointers receive pointers into `descriptor`, which we free.
    win32(unsafe { GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
        &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor) })?;
    let _descriptor = Local(descriptor);
    // A NULL DACL grants everyone full access.
    // Owner: the user, or SYSTEM/Administrators (an elevated administrator's
    // default owner for new objects is the Administrators group). These are
    // the same principals the DACL may grant; any other owner fails.
    if dacl.is_null() || !(user.equals(owner) || allowed.iter().any(|a| a.equals(owner))) { return Ok(false); }
    let mut info = ACL_SIZE_INFORMATION { AceCount: 0, AclBytesInUse: 0, AclBytesFree: 0 };
    // SAFETY: dacl points into the live descriptor; info is a correctly sized out-struct.
    if unsafe { GetAclInformation(dacl, &mut info as *mut _ as *mut c_void, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation) } == 0 {
        return Err(last_error());
    }
    for index in 0..info.AceCount {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: index < AceCount of a valid ACL; ace receives a pointer into the ACL.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 { return Err(last_error()); }
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        match header.AceType {
            ACCESS_DENIED_ACE_TYPE => continue, // denials only narrow access
            ACCESS_ALLOWED_ACE_TYPE => {
                // SAFETY: AceType says this is an ACCESS_ALLOWED_ACE; its SID starts at SidStart.
                let sid = unsafe { &(*(ace as *const ACCESS_ALLOWED_ACE)).SidStart } as *const u32 as PSID;
                if !(user.equals(sid) || allowed.iter().any(|a| a.equals(sid))) { return Ok(false); }
            }
            _ => return Ok(false), // object/callback/conditional ACEs: not expected here, refuse
        }
    }
    // Effective entries are what matters: a child inheriting from an owner-only
    // parent has the same set and is accepted.
    Ok(true)
}

pub(crate) fn ensure_owner_only(path: &Path) -> io::Result<()> {
    let user = current_user()?;
    let system = Sid::well_known(WinLocalSystemSid)?;
    let admins = Sid::well_known(WinBuiltinAdministratorsSid)?;
    let entry = |sid: &Sid, kind| EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL, grfAccessMode: SET_ACCESS, grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W { pMultipleTrustee: null_mut(), MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID, TrusteeType: kind, ptstrName: sid.ptr() as *mut u16 },
    };
    use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_WELL_KNOWN_GROUP;
    let entries = [entry(&user, TRUSTEE_IS_USER), entry(&system, TRUSTEE_IS_WELL_KNOWN_GROUP), entry(&admins, TRUSTEE_IS_WELL_KNOWN_GROUP)];
    let mut acl: *mut ACL = null_mut();
    // SAFETY: entries reference SIDs that live until the end of this function; acl receives a LocalAlloc'ed ACL.
    win32(unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null_mut(), &mut acl) })?;
    let _acl = Local(acl as *mut c_void);
    let name = wide(path);
    // SAFETY: name is NUL-terminated; owner SID and ACL are valid for the call.
    win32(unsafe { SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT,
        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
        user.ptr(), null_mut(), acl, null_mut()) })
}

pub(crate) fn file_id(handle: BorrowedHandle<'_>) -> io::Result<(u64, [u8; 16])> {
    // SAFETY: zero is a valid bit pattern for this plain-data struct.
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the duration of the borrow; info is a correctly sized out-struct.
    if unsafe { GetFileInformationByHandleEx(handle.as_raw_handle() as HANDLE, FileIdInfo, &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<FILE_ID_INFO>() as u32) } == 0 { return Err(last_error()); }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both names are NUL-terminated; without MOVEFILE_REPLACE_EXISTING the call fails if `to` exists.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 { return Err(last_error()); }
    Ok(())
}

/// Replace `to` with `from`, returning only after the move is on disk.
pub(crate) fn replace_durable(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both names are NUL-terminated; WRITE_THROUGH returns after the rename is flushed.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 { return Err(last_error()); }
    Ok(())
}

pub(crate) fn confine_current_process(limits: crate::Confinement) -> io::Result<()> {
    // SAFETY: anonymous job with default security; the handle is intentionally kept for the
    // process lifetime (leaked) so KILL_ON_JOB_CLOSE ends the process only when it exits.
    let job = unsafe { CreateJobObjectW(null_mut(), null_mut()) };
    if job.is_null() { return Err(last_error()); }
    // SAFETY: zero is a valid bit pattern for this plain-data struct.
    let mut extended: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    extended.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    extended.BasicLimitInformation.ActiveProcessLimit = 1;
    extended.ProcessMemoryLimit = limits.memory_bytes;
    // SAFETY: job is valid; the struct matches JobObjectExtendedLimitInformation.
    if unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, &extended as *const _ as *const c_void,
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) } == 0 { return Err(last_error()); }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS { UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
        | JOB_OBJECT_UILIMIT_EXITWINDOWS | JOB_OBJECT_UILIMIT_GLOBALATOMS | JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_READCLIPBOARD
        | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS | JOB_OBJECT_UILIMIT_WRITECLIPBOARD };
    // SAFETY: job is valid; the struct matches JobObjectBasicUIRestrictions.
    if unsafe { SetInformationJobObject(job, JobObjectBasicUIRestrictions, &ui as *const _ as *const c_void,
        std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32) } == 0 { return Err(last_error()); }
    // SAFETY: job and the current-process pseudo-handle are valid.
    if unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } == 0 { return Err(last_error()); }
    lower_integrity()
}

fn lower_integrity() -> io::Result<()> {
    let low = Sid::well_known(WinLowLabelSid)?;
    let token = process_token(TOKEN_ADJUST_DEFAULT | TOKEN_QUERY)?;
    let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: low.ptr(), Attributes: SE_GROUP_INTEGRITY } };
    let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + low.0.len() as u32;
    // SAFETY: token has TOKEN_ADJUST_DEFAULT; label references a valid SID living until return.
    // Lowering one's own integrity level is always permitted.
    if unsafe { SetTokenInformation(token.0, TokenIntegrityLevel, &label as *const _ as *const c_void, size) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!("paranoic-win-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir(&p).unwrap();
        p
    }

    #[test] fn acl_becomes_owner_only() {
        let dir = temp_dir();
        ensure_owner_only(&dir).unwrap();
        assert!(is_owner_only(&dir).unwrap());
        // A child inherits the same set of entries.
        std::fs::create_dir(dir.join("child")).unwrap();
        assert!(is_owner_only(&dir.join("child")).unwrap());
        // A system folder (other owner, Users can read) is not owner-only.
        let system_root = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()));
        assert!(!is_owner_only(&system_root).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test] fn rename_never_replaces() {
        let dir = temp_dir();
        std::fs::write(dir.join("a"), b"a").unwrap();
        std::fs::write(dir.join("b"), b"b").unwrap();
        assert!(rename_no_replace(&dir.join("a"), &dir.join("b")).is_err());
        assert_eq!(std::fs::read(dir.join("b")).unwrap(), b"b");
        rename_no_replace(&dir.join("a"), &dir.join("c")).unwrap();
        assert!(!dir.join("a").exists() && std::fs::read(dir.join("c")).unwrap() == b"a");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Child half of `confinement_blocks_processes_and_writes` (no-op unless re-executed).
    #[test] fn confinement_child() {
        let Ok(target) = std::env::var("PARANOIC_CONFINE_TARGET") else { return };
        confine_current_process(crate::Confinement { memory_bytes: 256 << 20 }).unwrap();
        let spawned = std::process::Command::new("cmd").args(["/C", "exit 0"]).status().is_ok();
        let wrote = std::fs::write(std::path::Path::new(&target).join("from-low-integrity.txt"), b"x").is_ok();
        std::process::exit(if spawned { 10 } else if wrote { 11 } else { 0 });
    }

    #[test] fn confinement_blocks_processes_and_writes() {
        let dir = temp_dir();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "imp::tests::confinement_child", "--nocapture", "--test-threads=1"])
            .env("PARANOIC_CONFINE_TARGET", &dir).status().unwrap();
        assert_eq!(status.code(), Some(0), "10 = could start a process, 11 = could write a user file");
        assert!(!dir.join("from-low-integrity.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test] fn file_ids_identify_objects() {
        let dir = temp_dir();
        std::fs::write(dir.join("a"), b"a").unwrap();
        std::fs::write(dir.join("b"), b"b").unwrap();
        let a = std::fs::File::open(dir.join("a")).unwrap();
        let a2 = std::fs::File::open(dir.join("a")).unwrap();
        let b = std::fs::File::open(dir.join("b")).unwrap();
        use std::os::windows::io::AsHandle;
        let (ida, idb, ida2) = (file_id(a.as_handle()).unwrap(), file_id(b.as_handle()).unwrap(), file_id(a2.as_handle()).unwrap());
        assert_eq!(ida, ida2); assert_ne!(ida, idb); assert_ne!(ida.1, [0u8; 16]);
        drop((a, a2, b));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
