//! Android isolated-service transport, configured before any document request.
use crate::*;
use paranoic_documents::worker_process::WorkerHost;

#[derive(Clone, uniffi::Record)]
pub struct HostWorkerOpened { pub error: u32, pub id: u64 }

#[uniffi::export(with_foreign)]
pub trait DocumentWorkers: Send + Sync {
    fn start(&self, kind: u8) -> HostWorkerOpened;
    fn receive(&self, id: u64, size: u32) -> HostBytes;
    fn send(&self, id: u64, data: Vec<u8>) -> u32;
    fn stop(&self, id: u64);
}
struct Adapter(Arc<dyn DocumentWorkers>);
fn status(code: u32) -> paranoic_core::Result<()> {
    if code == 0 { Ok(()) } else { Err(paranoic_core::CoreError::new(
        match code { 1 => paranoic_core::ErrorCode::PermissionDenied, 2 => paranoic_core::ErrorCode::Unsupported,
            4 => paranoic_core::ErrorCode::LimitExceeded, _ => paranoic_core::ErrorCode::Io }, "isolated worker host failed")) }
}
impl WorkerHost for Adapter {
    fn start(&self, kind: u8) -> paranoic_core::Result<u64> {
        let opened = self.0.start(kind); status(opened.error)?; Ok(opened.id)
    }
    fn read(&self, id: u64, size: u32) -> paranoic_core::Result<Vec<u8>> {
        let reply = self.0.receive(id, size); status(reply.error)?; Ok(reply.data)
    }
    fn write(&self, id: u64, data: Vec<u8>) -> paranoic_core::Result<()> { status(self.0.send(id, data)) }
    fn stop(&self, id: u64) { self.0.stop(id); }
}
#[uniffi::export]
impl NativeRuntime {
    /// A missing Android host is an error at use, never a same-UID fallback.
    pub fn document_workers_configure(&self, host: Option<Arc<dyn DocumentWorkers>>) -> Result<()> {
        // Close the cached session before replacing its launch context.
        self.pdf.lock().map_err(lock_error)?.take();
        *self.document_workers.lock().map_err(lock_error)? = host.map(|h| Arc::new(Adapter(h)) as Arc<dyn WorkerHost>);
        Ok(())
    }
}
impl NativeRuntime {
    pub(crate) fn document_worker_host(&self) -> Result<Option<Arc<dyn WorkerHost>>> {
        Ok(self.document_workers.lock().map_err(lock_error)?.clone())
    }
}
