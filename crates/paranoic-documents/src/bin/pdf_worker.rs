//! Out-of-process worker for hostile formats: PDFium (`--pdfium <lib>`) and
//! 7z decoding (`--sevenz`, see `sevenz_isolated`). Out-of-process PDFium worker. Started by `pdf_client::PdfSession` with a
//! cleared environment; speaks a length-prefixed binary protocol on
//! stdin/stdout and never touches files except the PDFium library itself.
//! A crash or exploit here cannot read the application's memory (Core
//! sessions, keys). Exits on EOF, protocol error or QUIT.
//!
//! Requests: LOAD u64 len + bytes (first, once); 1 RENDER u32 page u32 width;
//! 2 SEARCH u32 start u32 len + UTF-8 query; 3 QUIT.
//! Reply: u8 status (0 ok, 1 invalid, 2 limit, 3 unavailable) + u32 len + payload.
use std::io::{Read, Write};
use paranoic_documents::pdf;

fn u32_in(input: &mut impl Read) -> Option<u32> { let mut b = [0u8; 4]; input.read_exact(&mut b).ok()?; Some(u32::from_le_bytes(b)) }
fn reply(output: &mut impl Write, status: u8, payload: &[u8]) -> bool {
    let len = payload.len() as u32;
    output.write_all(&[status]).is_ok() && output.write_all(&len.to_le_bytes()).is_ok()
        && output.write_all(payload).is_ok() && output.flush().is_ok()
}
fn failure(f: pdf::Failure) -> u8 { match f { pdf::Failure::Invalid => 1, pdf::Failure::Limit => 2 } }

fn main() {
    #[cfg(target_os = "android")]
    if sandbox::watch_parent().is_err() { return; }
    // 7z mode: no files needed at all; confine before reading anything.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--sevenz") {
        if sandbox::enter(1800).is_err() { return; }
        paranoic_documents::sevenz_isolated::serve_worker(std::io::stdin().lock(), std::io::stdout().lock());
        return;
    }
    let mut args = std::env::args_os().skip(1);
    let (Some(flag), Some(library)) = (args.next(), args.next()) else { return };
    if flag != "--pdfium" { return; }
    let stdin = std::io::stdin(); let stdout = std::io::stdout();
    let (mut input, mut output) = (stdin.lock(), stdout.lock());
    let mut length = [0u8; 8];
    if input.read_exact(&mut length).is_err() { return; }
    let length = u64::from_le_bytes(length);
    if length > pdf::MAX_INPUT { let _ = reply(&mut output, 2, &[]); return; }
    let mut data = zeroize::Zeroizing::new(vec![0u8; length as usize]);
    if input.read_exact(&mut data).is_err() { return; }
    let Some(pdfium) = pdf::library(std::path::Path::new(&library)) else { let _ = reply(&mut output, 3, &[]); return; };
    // Everything that needs the filesystem is done: confine the process before
    // the first byte of the untrusted PDF is parsed. Fail closed.
    if sandbox::enter(120).is_err() { let _ = reply(&mut output, 3, &[]); return; }
    let Ok(document) = pdfium.load_pdf_from_byte_slice(&data, None) else { let _ = reply(&mut output, 1, &[]); return; };
    let count = pdf::page_count(&document);
    if count > 20_000 { let _ = reply(&mut output, 2, &[]); return; }
    if !reply(&mut output, 0, &count.to_le_bytes()) { return; }
    loop {
        let mut tag = [0u8; 1];
        if input.read_exact(&mut tag).is_err() { return; }
        let ok = match tag[0] {
            1 => {
                let (Some(page), Some(width)) = (u32_in(&mut input), u32_in(&mut input)) else { return };
                match pdf::render(&document, page, width) { Ok(png) => reply(&mut output, 0, &png), Err(f) => reply(&mut output, failure(f), &[]) }
            }
            2 => {
                let (Some(start), Some(len)) = (u32_in(&mut input), u32_in(&mut input)) else { return };
                if len > 1024 { return; }
                let mut query = zeroize::Zeroizing::new(vec![0u8; len as usize]);
                if input.read_exact(&mut query).is_err() { return; }
                let Ok(query) = std::str::from_utf8(&query) else { let _ = reply(&mut output, 1, &[]); continue };
                match pdf::search(&document, query, start) {
                    Ok((pages, next)) => {
                        let mut payload = next.unwrap_or(u32::MAX).to_le_bytes().to_vec();
                        for p in pages { payload.extend_from_slice(&p.to_le_bytes()); }
                        reply(&mut output, 0, &payload)
                    }
                    Err(f) => reply(&mut output, failure(f), &[]),
                }
            }
            _ => return,
        };
        if !ok { return; }
    }
}

/// Linux/Android self-confinement (seccomp + rlimits). Only memory management,
/// existing pipe I/O, time, signal bookkeeping and process exit are allowed.
/// Everything else, including unknown/new syscalls, fails with EPERM. The
/// filter is synchronized across all threads; failure refuses decoding.
/// Limits: 1 GiB data segment, CPU 120 s (PDF) / 1800 s (7z), no file growth/core dumps.
/// Windows uses the job/Low-integrity policy below; AppContainer remains open.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod sandbox {
    use std::collections::BTreeMap;
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};
    #[cfg(target_os = "android")]
    pub fn watch_parent() -> Result<(), ()> {
        use rustix::process::{getppid, set_parent_process_death_signal, Pid, Signal};
        let parent = getppid();
        if parent.is_none() || parent == Some(Pid::INIT) { return Err(()); }
        // The Android service's main thread is the process creator. A service
        // crash/LMK must kill a decoder even while it is computing, not only
        // when it next notices EOF. Refuse the parent-exited-before-prctl race.
        set_parent_process_death_signal(Some(Signal::Kill)).map_err(|_| ())?;
        if getppid() != parent { return Err(()); }
        Ok(())
    }
    /// fstat by descriptor only. The `libc` crate names it for x86_64, but not
    /// for Android arm64, where the kernel's generic table numbers it 80 (as
    /// glibc's aarch64 `SYS_fstat`). `newfstatat` is deliberately not allowed:
    /// it can look up paths.
    #[cfg(not(all(target_os = "android", target_arch = "aarch64")))]
    const SYS_FSTAT: i64 = libc::SYS_fstat;
    #[cfg(all(target_os = "android", target_arch = "aarch64"))]
    const SYS_FSTAT: i64 = 80;
    pub fn enter(cpu_seconds: u64) -> Result<(), ()> {
        use rustix::process::{setrlimit, Resource, Rlimit};
        for (resource, value) in [(Resource::Data, 1u64 << 30), (Resource::Cpu, cpu_seconds),
            (Resource::Fsize, 0), (Resource::Core, 0)] {
            setrlimit(resource, Rlimit { current: Some(value), maximum: Some(value) }).map_err(|_| ())?;
        }
        // No path lookup/open, network, fork/clone, exec, ptrace, namespace,
        // descriptor reception, cross-process memory or signaling syscalls.
        // PDFium is loaded before entry; font files cannot be opened afterwards.
        let allowed: Vec<i64> = vec![
            libc::SYS_read, libc::SYS_write, libc::SYS_readv, libc::SYS_writev,
            libc::SYS_close, SYS_FSTAT, libc::SYS_lseek,
            libc::SYS_brk, libc::SYS_mmap, libc::SYS_munmap, libc::SYS_mremap,
            libc::SYS_mprotect, libc::SYS_madvise,
            libc::SYS_futex, libc::SYS_sched_yield,
            libc::SYS_clock_gettime, libc::SYS_gettimeofday,
            libc::SYS_nanosleep, libc::SYS_clock_nanosleep, libc::SYS_restart_syscall,
            libc::SYS_rt_sigaction, libc::SYS_rt_sigprocmask, libc::SYS_rt_sigreturn, libc::SYS_sigaltstack,
            libc::SYS_getpid, libc::SYS_gettid, libc::SYS_getuid, libc::SYS_geteuid,
            libc::SYS_getgid, libc::SYS_getegid, libc::SYS_getrandom,
            libc::SYS_exit, libc::SYS_exit_group,
        ];
        let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = allowed.into_iter().map(|n| (n, Vec::new())).collect();
        let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|_| ())?;
        let filter = SeccompFilter::new(rules, SeccompAction::Errno(libc::EPERM as u32), SeccompAction::Allow, arch).map_err(|_| ())?;
        let program: BpfProgram = filter.try_into().map_err(|_| ())?;
        seccompiler::apply_filter_all_threads(&program).map_err(|_| ())
    }
}
/// Windows is confined at creation by the parent (AppContainer + limited job).
/// Refuse ordinary same-user launches; there is no Low-integrity-only fallback.
#[cfg(windows)]
mod sandbox {
    pub fn enter(_cpu_seconds: u64) -> Result<(), ()> {
        if paranoic_windows::document_worker_is_confined().map_err(|_| ())? { Ok(()) } else { Err(()) }
    }
}
#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
mod sandbox {
    /// Audit A23: no confinement exists for this platform, so the worker refuses to run and the
    /// app reports PDF/7z as unavailable instead of decoding hostile files unconfined.
    pub fn enter(_cpu_seconds: u64) -> Result<(), ()> { Err(()) }
}

