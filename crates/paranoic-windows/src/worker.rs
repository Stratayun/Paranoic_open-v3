//! Parent-side AppContainer launch. No user-data paths or network capabilities
//! are granted. Worker resources must be installed with AppContainer read/exec
//! access; an inaccessible install fails closed, never by widening a user ACL.
use std::{ffi::{c_void, OsStr}, fs::{File, OpenOptions}, io, mem::{size_of, size_of_val},
    os::windows::{ffi::OsStrExt, io::{AsRawHandle, FromRawHandle, OwnedHandle}},
    path::Path, ptr::{null, null_mut}, sync::atomic::{AtomicU64, Ordering}};
use windows_sys::Win32::{
    Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::{FreeSid, GetTokenInformation, TokenIsAppContainer, TOKEN_QUERY, PSID,
        SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile}},
    System::{Pipes::CreatePipe,
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, SetInformationJobObject, TerminateJobObject,
            JobObjectExtendedLimitInformation, JobObjectBasicUIRestrictions, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOBOBJECT_BASIC_UI_RESTRICTIONS, JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
            JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
            JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION},
        Threading::{CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, InitializeProcThreadAttributeList,
            OpenProcessToken, ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
            LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW}},
};

fn error() -> io::Error { io::Error::last_os_error() }
fn raw(handle: &OwnedHandle) -> HANDLE { handle.as_raw_handle() as HANDLE }
fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle as isize == -1 { return Err(error()); }
    // SAFETY: callers transfer a fresh, valid kernel handle; it is owned exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut out: Vec<u16> = value.encode_wide().collect();
    if out.contains(&0) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in worker path")); }
    out.push(0); Ok(out)
}
fn quoted(value: &OsStr) -> io::Result<Vec<u16>> {
    let chars = wide(value)?;
    let mut out = vec![b'"' as u16];
    let mut slashes = 0;
    for &c in &chars[..chars.len() - 1] {
        if c == b'\\' as u16 { slashes += 1; continue; }
        out.extend(std::iter::repeat_n(b'\\' as u16, if c == b'"' as u16 { slashes * 2 + 1 } else { slashes }));
        slashes = 0; out.push(c);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16); Ok(out)
}
struct Profile { name: Vec<u16> }
impl Drop for Profile {
    fn drop(&mut self) {
        // SAFETY: name is an owned NUL-terminated profile created by this launch.
        unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
    }
}
struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: CreateAppContainerProfile allocated this SID with FreeSid ownership.
        unsafe { FreeSid(self.0) };
    }
}
struct Attributes { _buffer: Vec<usize>, pointer: LPPROC_THREAD_ATTRIBUTE_LIST }
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented size query, null attribute storage, two attributes.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes) };
        if bytes == 0 || bytes > 1 << 20 { return Err(error()); }
        let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let pointer = buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        // SAFETY: aligned buffer has the requested writable size and lives with the guard.
        if unsafe { InitializeProcThreadAttributeList(pointer, 2, 0, &mut bytes) } == 0 { return Err(error()); }
        Ok(Self { _buffer: buffer, pointer })
    }
    fn set<T>(&mut self, kind: usize, value: &T, bytes: usize) -> io::Result<()> {
        // SAFETY: the caller supplies a live struct/array of `bytes`; it remains live through CreateProcessW.
        if unsafe { UpdateProcThreadAttribute(self.pointer, 0, kind, value as *const T as *const c_void,
            bytes, null_mut(), null()) } == 0 { return Err(error()); }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: this list was successfully initialized and its buffer is still live.
        unsafe { DeleteProcThreadAttributeList(self.pointer) };
    }
}
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    let (mut read, mut write) = (null_mut(), null_mut());
    // SAFETY: valid out-pointers and security attributes; both new handles become owned below.
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 64 * 1024) } == 0 { return Err(error()); }
    Ok((owned(read)?, owned(write)?))
}
fn no_inherit(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: live handle; changes only its inheritance flag in this process.
    if unsafe { SetHandleInformation(raw(handle), HANDLE_FLAG_INHERIT, 0) } == 0 { return Err(error()); }
    Ok(())
}

/// Owns the process, its job and private AppContainer profile. Drop kills the
/// entire job before removing the profile; no child escapes a failed setup.
pub struct SandboxedWorker {
    process: OwnedHandle, job: OwnedHandle, _profile: Profile,
    pub stdin: Option<File>, pub stdout: Option<File>,
}
impl SandboxedWorker {
    pub fn kill(&mut self) -> io::Result<()> {
        // SAFETY: live job created by this launcher; it contains only this worker.
        if unsafe { TerminateJobObject(raw(&self.job), 1) } == 0 { return Err(error()); }
        Ok(())
    }
    pub fn wait(&mut self) -> io::Result<()> {
        // SAFETY: process is a live synchronization handle; the wait is bounded.
        match unsafe { WaitForSingleObject(raw(&self.process), 5000) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(io::Error::new(io::ErrorKind::TimedOut, "worker termination pending")),
            _ => Err(error()),
        }
    }
}
impl Drop for SandboxedWorker { fn drop(&mut self) { let _ = self.kill(); let _ = self.wait(); } }

/// Start suspended, with zero capabilities, an exact stdin/stdout handle list,
/// a one-process/memory-limited job, and an empty environment. Resume only
/// after every restriction has succeeded. No ordinary CreateProcess fallback.
pub fn spawn_document_worker(executable: &Path, arguments: &[&OsStr]) -> io::Result<SandboxedWorker> {
    if !executable.is_absolute() { return Err(io::Error::new(io::ErrorKind::InvalidInput, "absolute worker path required")); }
    let exe = wide(executable.as_os_str())?;
    let directory = wide(executable.parent().ok_or_else(|| io::Error::other("worker directory missing"))?.as_os_str())?;
    let mut command = quoted(executable.as_os_str())?;
    for argument in arguments { command.push(b' ' as u16); command.extend(quoted(argument)?); }
    command.push(0);
    if command.len() > 32767 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "worker command too long")); }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| io::Error::other("invalid system clock"))?.as_nanos() as u64;
    let name = format!("Paranoic.Decoder.{:x}.{stamp:x}.{:x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let name = wide(OsStr::new(&name))?;
    let mut sid: PSID = null_mut();
    // SAFETY: all strings are NUL-terminated, capability list is empty, SID out-pointer is writable.
    let status = unsafe { CreateAppContainerProfile(name.as_ptr(), name.as_ptr(), name.as_ptr(), null(), 0, &mut sid) };
    if status < 0 { return Err(io::Error::other(format!("AppContainer creation failed: {status:#x}"))); }
    let profile = Profile { name };
    if sid.is_null() { return Err(io::Error::other("AppContainer returned no SID")); }
    let sid = Sid(sid);
    let capabilities = SECURITY_CAPABILITIES { AppContainerSid: sid.0, Capabilities: null_mut(), CapabilityCount: 0, Reserved: 0 };
    let (child_input, parent_input) = pipe()?;
    let (parent_output, child_output) = pipe()?;
    no_inherit(&parent_input)?; no_inherit(&parent_output)?;
    let stderr = OpenOptions::new().write(true).open("NUL")?;
    // SAFETY: live NUL-device handle owned here; only the explicit handle list inherits it.
    if unsafe { SetHandleInformation(stderr.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 { return Err(error()); }
    let inherited = [raw(&child_input), raw(&child_output), stderr.as_raw_handle() as HANDLE];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize, &capabilities, size_of::<SECURITY_CAPABILITIES>())?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &inherited, size_of_val(&inherited))?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = raw(&child_input);
    startup.StartupInfo.hStdOutput = raw(&child_output);
    startup.StartupInfo.hStdError = stderr.as_raw_handle() as HANDLE;
    startup.lpAttributeList = attributes.pointer;
    let environment = [0u16; 2];
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: all pointed-to buffers/attributes stay live through this call; the command is writable.
    if unsafe { CreateProcessW(exe.as_ptr(), command.as_mut_ptr(), null(), null(), 1,
        CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
        environment.as_ptr() as *const c_void, directory.as_ptr(), &startup.StartupInfo, &mut process) } == 0 { return Err(error()); }
    let process_handle = owned(process.hProcess)?;
    let thread = owned(process.hThread)?;
    // A failure before assignment must terminate the still-suspended process.
    struct Suspended<'a> { process: &'a OwnedHandle, armed: bool }
    impl Drop for Suspended<'_> {
        fn drop(&mut self) { if self.armed {
            // SAFETY: the guard owns the not-yet-resumed child; no other process is affected.
            unsafe { TerminateProcess(raw(self.process), 1) };
        } }
    }
    let mut suspended = Suspended { process: &process_handle, armed: true };
    // SAFETY: creates a new anonymous job; returned handle is transferred to RAII.
    let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = 1 << 30;
    // SAFETY: job and matching initialized limit structure are live.
    if unsafe { SetInformationJobObject(raw(&job), JobObjectExtendedLimitInformation,
        &limits as *const _ as *const c_void, size_of_val(&limits) as u32) } == 0 { return Err(error()); }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS { UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
        | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS | JOB_OBJECT_UILIMIT_EXITWINDOWS | JOB_OBJECT_UILIMIT_GLOBALATOMS
        | JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_READCLIPBOARD | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
        | JOB_OBJECT_UILIMIT_WRITECLIPBOARD };
    // SAFETY: live job and correctly sized UI restriction structure, before child assignment.
    if unsafe { SetInformationJobObject(raw(&job), JobObjectBasicUIRestrictions,
        &ui as *const _ as *const c_void, size_of_val(&ui) as u32) } == 0 { return Err(error()); }
    // SAFETY: child remains suspended and both handles belong to this launch.
    if unsafe { AssignProcessToJobObject(raw(&job), raw(&process_handle)) } == 0 { return Err(error()); }
    // SAFETY: this is the initial thread of the suspended, fully confined child.
    if unsafe { ResumeThread(raw(&thread)) } == u32::MAX { return Err(error()); }
    suspended.armed = false; drop(suspended);
    drop((child_input, child_output));
    Ok(SandboxedWorker { process: process_handle, job, _profile: profile,
        stdin: Some(File::from(parent_input)), stdout: Some(File::from(parent_output)) })
}

/// Worker-side admission check: an ordinary same-user process is refused.
pub fn document_worker_is_confined() -> io::Result<bool> {
    // SAFETY: returns the pseudo-handle for this process.
    let process = unsafe { GetCurrentProcess() };
    let mut token = null_mut();
    // SAFETY: valid process pseudo-handle and a writable new-handle out-pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 { return Err(error()); }
    let token = owned(token)?;
    let (mut flag, mut size) = (0u32, 0u32);
    // SAFETY: TokenIsAppContainer returns one DWORD into the correctly sized buffer.
    if unsafe { GetTokenInformation(raw(&token), TokenIsAppContainer, &mut flag as *mut _ as *mut c_void,
        size_of::<u32>() as u32, &mut size) } == 0 { return Err(error()); }
    let mut in_job = 0;
    // SAFETY: queries current-process membership; a null job means any job.
    if unsafe { IsProcessInJob(process, null_mut(), &mut in_job) } == 0 { return Err(error()); }
    Ok(flag != 0 && in_job != 0)
}
