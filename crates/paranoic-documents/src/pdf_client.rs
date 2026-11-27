//! Application side of PDF viewing. PDFium (C++, hostile input) runs in a
//! separate OS process (`paranoic-pdf-worker`): the document is read through
//! Core (length/metadata checked), sent once over a pipe, and pages/search
//! results come back as bounded replies. Every reply has a deadline; on
//! timeout, crash or protocol error the worker is killed and the session is
//! unusable. The application process never maps PDFium.
//!
//! Windows launches directly in AppContainer; Android requires an
//! isolatedProcess service and never falls back to the application's UID.
//! Linux uses the worker's syscall allowlist. Only PDFium's installation path
//! and document bytes are supplied, with no inherited application environment.
use std::{io::{Read, Write}, path::Path, sync::{mpsc, Arc}, time::Duration};
use crate::worker_process::{WorkerHost, WorkerProcess};
use paranoic_core::{Core, EntryKind, Location, Metadata};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Result};

const MAX_INPUT: u64 = 64 << 20;
/// Largest PDF a session accepts.
pub const PDF_INPUT_LIMIT: u64 = MAX_INPUT;
const MAX_REPLY: u32 = 64 << 20;
const DEADLINE: Duration = Duration::from_secs(30);

/// Requests go to a writer thread (bounded), replies come from a reader thread
/// through a one-slot channel, so a misbehaving worker can neither block the
/// app on a pipe write nor queue unbounded replies. Every request has one
/// deadline covering write and reply; on expiry the worker is killed.
pub struct PdfSession {
    child: WorkerProcess, requests: mpsc::SyncSender<Zeroizing<Vec<u8>>>,
    replies: mpsc::Receiver<Option<(u8, Vec<u8>)>>, pages: u32, source: Metadata, dead: bool,
}

fn unavailable() -> CoreError { CoreError::new(ErrorCode::Unsupported, "PDF worker is unavailable") }
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "PDF cannot be processed") }
fn limit() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "PDF resource limit exceeded") }

impl PdfSession {
    /// Read the PDF through Core and start a worker for it.
    pub fn open(core: &Core, location: &Location, worker: &Path, pdfium: &Path) -> Result<Self> {
        Self::open_with_host(core, location, worker, pdfium, None)
    }
    pub fn open_with_host(core: &Core, location: &Location, worker: &Path, pdfium: &Path,
        host: Option<Arc<dyn WorkerHost>>) -> Result<Self> {
        if !worker.is_absolute() || !pdfium.is_absolute() { return Err(unavailable()); }
        let mut reader = core.open_read(location)?;
        let before = reader.metadata()?;
        if before.kind != EntryKind::File || before.bytes > MAX_INPUT { return Err(limit()); }
        let mut data = Zeroizing::new(Vec::with_capacity(before.bytes as usize));
        reader.by_ref().take(MAX_INPUT + 1).read_to_end(&mut data).map_err(CoreError::io)?;
        if data.len() as u64 != before.bytes || reader.metadata()? != before {
            return Err(CoreError::new(ErrorCode::SourceChanged, "PDF changed while reading"));
        }
        Self::open_bytes_with_host(data, before, worker, pdfium, host)
    }
    /// Start a worker for a PDF already read and authenticated in memory
    /// (inside an encrypted vault, E4.7). `source`: the version it came from,
    /// used by callers to decide whether the session can be reused.
    pub fn open_bytes_with_host(data: Zeroizing<Vec<u8>>, source: Metadata, worker: &Path, pdfium: &Path,
        host: Option<Arc<dyn WorkerHost>>) -> Result<Self> {
        if !worker.is_absolute() || !pdfium.is_absolute() { return Err(unavailable()); }
        if data.len() as u64 > MAX_INPUT { return Err(limit()); }
        let before = source;
        if !data.starts_with(b"%PDF-") { return Err(invalid()); }
        let mut child = WorkerProcess::spawn(worker, Some(pdfium), host)?;
        let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else { let _ = child.kill(); return Err(unavailable()); };
        let (sender, replies) = mpsc::sync_channel(1);
        let (requests, outgoing) = mpsc::sync_channel::<Zeroizing<Vec<u8>>>(1);
        std::thread::Builder::new().name("paranoic-pdf-request".into()).spawn(move || {
            // Ends when the session drops the sender or the pipe breaks (worker killed).
            for message in outgoing { if stdin.write_all(&message).and_then(|_| stdin.flush()).is_err() { return; } }
        }).map_err(|_| { let _ = child.kill(); unavailable() })?;
        std::thread::Builder::new().name("paranoic-pdf-reply".into()).spawn(move || loop {
            let mut head = [0u8; 5];
            if stdout.read_exact(&mut head).is_err() { let _ = sender.send(None); return; }
            let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
            // Audit A22: only a success reply may be large; error replies carry no payload of note.
            let limit = match head[0] { 0 => MAX_REPLY, 1 | 2 => 64, _ => { let _ = sender.send(None); return; } };
            if len > limit { let _ = sender.send(None); return; }
            let mut payload = vec![0u8; len as usize];
            if stdout.read_exact(&mut payload).is_err() || sender.send(Some((head[0], payload))).is_err() { return; }
        }).map_err(|_| { let _ = child.kill(); unavailable() })?;
        let mut session = Self { child, requests, replies, pages: 0, source: before, dead: false };
        let mut load = Zeroizing::new(Vec::with_capacity(data.len() + 8));
        load.extend_from_slice(&(data.len() as u64).to_le_bytes()); load.extend_from_slice(&data);
        drop(data);
        let reply = session.request(load)?;
        if reply.len() != 4 { session.kill(); return Err(unavailable()); }
        session.pages = u32::from_le_bytes([reply[0], reply[1], reply[2], reply[3]]);
        Ok(session)
    }
    pub fn pages(&self) -> u32 { self.pages }
    /// Metadata of the source when it was loaded (callers reuse a session only
    /// while the file is unchanged).
    pub fn source(&self) -> &Metadata { &self.source }
    pub fn is_alive(&self) -> bool { !self.dead }
    fn kill(&mut self) { self.dead = true; let _ = self.child.kill(); let _ = self.child.wait(); }
    /// One request, one reply, one deadline. A reply that arrives without a
    /// request (or a second one) is a protocol violation.
    fn request(&mut self, message: Zeroizing<Vec<u8>>) -> Result<Vec<u8>> {
        if self.dead { return Err(unavailable()); }
        if !matches!(self.replies.try_recv(), Err(mpsc::TryRecvError::Empty)) { self.kill(); return Err(unavailable()); }
        let deadline = std::time::Instant::now() + DEADLINE;
        let mut pending = Some(message);
        while let Some(message) = pending.take() {
            match self.requests.try_send(message) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(message)) => {
                    if std::time::Instant::now() >= deadline { self.kill(); return Err(unavailable()); }
                    std::thread::sleep(Duration::from_millis(5));
                    pending = Some(message);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => { self.kill(); return Err(unavailable()); }
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match self.replies.recv_timeout(remaining) {
            Ok(Some((0, payload))) => Ok(payload),
            Ok(Some((1, _))) => Err(invalid()),
            Ok(Some((2, _))) => Err(limit()),
            _ => { self.kill(); Err(unavailable()) }
        }
    }
    pub fn render(&mut self, page: u32, width: u32) -> Result<Vec<u8>> {
        if page >= self.pages || !(128..=4096).contains(&width) { return Err(limit()); }
        let mut message = Zeroizing::new(vec![1u8]); message.extend_from_slice(&page.to_le_bytes()); message.extend_from_slice(&width.to_le_bytes());
        let png = self.request(message)?;
        // Check signature and IHDR dimensions before anyone decodes it.
        let dims = (png.len() >= 24 && png.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) && &png[12..16] == b"IHDR")
            .then(|| (u32::from_be_bytes([png[16], png[17], png[18], png[19]]), u32::from_be_bytes([png[20], png[21], png[22], png[23]])));
        match dims {
            Some((w, h)) if w > 0 && h > 0 && w <= width && h <= 4096 => Ok(png),
            _ => { self.kill(); Err(unavailable()) }
        }
    }
    /// Pages in [start, start+16) containing `query`; `next` continues the scan.
    pub fn search(&mut self, query: &str, start: u32) -> Result<(Vec<u32>, Option<u32>)> {
        if query.is_empty() || query.len() > 1024 || start > self.pages { return Err(invalid()); }
        let mut message = Zeroizing::new(vec![2u8]);
        message.extend_from_slice(&start.to_le_bytes()); message.extend_from_slice(&(query.len() as u32).to_le_bytes());
        message.extend_from_slice(query.as_bytes());
        let payload = self.request(message)?;
        if payload.len() < 4 || payload.len() % 4 != 0 { self.kill(); return Err(unavailable()); }
        let words: Vec<u32> = payload.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let next = (words[0] != u32::MAX).then_some(words[0]);
        if next.is_some_and(|n| n > self.pages) || words[1..].iter().any(|p| *p >= self.pages) { self.kill(); return Err(unavailable()); }
        Ok((words[1..].to_vec(), next))
    }
}
impl Drop for PdfSession {
    /// Never waits on the pipe: the worker holds no state worth a clean exit.
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

