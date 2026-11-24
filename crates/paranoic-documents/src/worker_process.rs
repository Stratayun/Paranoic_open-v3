//! Platform-confined worker transport. Protocol callers retain their existing
//! message limits/deadlines; Android must provide an isolated-service host.
use std::{io::{self, Read, Write}, path::Path, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use crate::{CoreError, ErrorCode, Result};

/// Android host: 1 PDF, 2 7z. Carries only bounded bytes, never document paths,
/// grants, journal handles or keys. `stop` must revoke both pipe directions.
pub trait WorkerHost: Send + Sync {
    fn start(&self, kind: u8) -> Result<u64>;
    fn read(&self, id: u64, size: u32) -> Result<Vec<u8>>;
    fn write(&self, id: u64, data: Vec<u8>) -> Result<()>;
    fn stop(&self, id: u64);
}
fn unavailable() -> CoreError { CoreError::new(ErrorCode::Unsupported, "isolated document worker unavailable") }

enum Backend {
    #[cfg(not(any(windows, target_os = "android")))] Native(std::process::Child),
    #[cfg(windows)] Windows(paranoic_windows::SandboxedWorker),
    Host(Arc<HostControl>),
}
struct HostControl { host: Arc<dyn WorkerHost>, id: u64, stopped: AtomicBool }
impl HostControl {
    fn stop(&self) { if !self.stopped.swap(true, Ordering::AcqRel) { self.host.stop(self.id); } }
    fn check(&self) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) { Err(io::Error::new(io::ErrorKind::BrokenPipe, "worker stopped")) } else { Ok(()) }
    }
}
struct HostRead(Arc<HostControl>);
impl Read for HostRead {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.0.check()?;
        if output.is_empty() { return Ok(0); }
        let count = output.len().min(1 << 20);
        let data = zeroize::Zeroizing::new(self.0.host.read(self.0.id, count as u32).map_err(|_| io::Error::other("worker read failed"))?);
        self.0.check()?;
        if data.len() > count { return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized worker reply")); }
        output[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}
struct HostWrite(Arc<HostControl>);
impl Write for HostWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.check()?;
        let count = bytes.len().min(1 << 20);
        self.0.host.write(self.0.id, bytes[..count].to_vec()).map_err(|_| io::Error::other("worker write failed"))?;
        self.0.check()?; Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> { self.0.check() }
}

pub struct WorkerProcess {
    backend: Backend,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Option<Box<dyn Read + Send>>,
}
impl WorkerProcess {
    pub fn spawn(worker: &Path, pdfium: Option<&Path>, host: Option<Arc<dyn WorkerHost>>) -> Result<Self> {
        if !worker.is_absolute() || pdfium.is_some_and(|p| !p.is_absolute()) { return Err(unavailable()); }
        if let Some(host) = host {
            if !cfg!(target_os = "android") { return Err(unavailable()); }
            let id = host.start(if pdfium.is_some() { 1 } else { 2 })?;
            if id == 0 { return Err(unavailable()); }
            let control = Arc::new(HostControl { host, id, stopped: AtomicBool::new(false) });
            return Ok(Self { backend: Backend::Host(Arc::clone(&control)),
                stdin: Some(Box::new(HostWrite(Arc::clone(&control)))), stdout: Some(Box::new(HostRead(control))) });
        }
        // Never run a same-UID executable as an Android fallback.
        #[cfg(target_os = "android")]
        { Err(unavailable()) }
        #[cfg(windows)]
        {
            let args = match pdfium { Some(lib) => vec![std::ffi::OsStr::new("--pdfium"), lib.as_os_str()],
                None => vec![std::ffi::OsStr::new("--sevenz")] };
            let mut child = paranoic_windows::spawn_document_worker(worker, &args).map_err(|_| unavailable())?;
            let stdin = child.stdin.take().map(|p| Box::new(p) as Box<dyn Write + Send>);
            let stdout = child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>);
            Ok(Self { backend: Backend::Windows(child), stdin, stdout })
        }
        #[cfg(not(any(windows, target_os = "android")))]
        {
            use std::process::{Command, Stdio};
            let mut command = Command::new(worker);
            match pdfium { Some(lib) => { command.arg("--pdfium").arg(lib); }, None => { command.arg("--sevenz"); } }
            command.env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
            if let Some(dir) = worker.parent() { command.current_dir(dir); }
            let mut child = command.spawn().map_err(|_| unavailable())?;
            let stdin = child.stdin.take().map(|p| Box::new(p) as Box<dyn Write + Send>);
            let stdout = child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>);
            Ok(Self { backend: Backend::Native(child), stdin, stdout })
        }
    }
    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.backend {
            #[cfg(not(any(windows, target_os = "android")))] Backend::Native(c) => c.kill(),
            #[cfg(windows)] Backend::Windows(c) => c.kill(),
            Backend::Host(c) => { c.stop(); Ok(()) },
        }
    }
    pub fn wait(&mut self) -> io::Result<()> {
        match &mut self.backend {
            #[cfg(not(any(windows, target_os = "android")))] Backend::Native(c) => c.wait().map(|_| ()),
            #[cfg(windows)] Backend::Windows(c) => c.wait(),
            Backend::Host(_) => Ok(()),
        }
    }
}
impl Drop for WorkerProcess { fn drop(&mut self) { let _ = self.kill(); let _ = self.wait(); } }
