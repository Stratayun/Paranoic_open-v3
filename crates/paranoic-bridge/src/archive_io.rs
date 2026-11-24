//! Read-only preparation I/O with cancellable waits. A provider cannot be
//! forcibly interrupted safely; its thread keeps its slot until it really
//! returns. At most two such threads exist, including cancelled/hung reads.
use crate::archive_work::ArchiveControl;
use paranoic_core::{CoreError, EntryKind, ErrorCode, Metadata, ResourcePath, Result, Storage};
use std::{io::{self, Read}, sync::{Arc, Condvar, Mutex, OnceLock, mpsc}, time::{Duration, Instant}};
use zeroize::Zeroizing;

static BUSY: Mutex<usize> = Mutex::new(0);
static FREE: Condvar = Condvar::new();
const SLOTS: usize = 2;
const TICK: Duration = Duration::from_millis(50);
const IO_DEADLINE: Duration = Duration::from_secs(30);
type Task = Box<dyn FnOnce() + Send>;
static POOL: OnceLock<Result<mpsc::SyncSender<Task>>> = OnceLock::new();
fn failed() -> CoreError { CoreError::new(ErrorCode::Io, "archive source I/O unavailable") }
fn timeout() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "archive source I/O deadline exceeded") }
fn dispatch(task: Task) -> Result<()> {
    let sender = POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Task>(SLOTS);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..SLOTS {
            let receiver = Arc::clone(&receiver);
            std::thread::Builder::new().name(format!("paranoic-archive-io-{index}")).spawn(move || loop {
                let next = {
                    let receiver = match receiver.lock() { Ok(r) => r, Err(_) => return };
                    receiver.recv()
                };
                let Ok(task) = next else { return };
                // Read-only tasks drop their reply channel on panic, so the
                // caller fails; one bad provider must not shrink the pool.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
            }).map_err(|_| failed())?;
        }
        Ok(sender)
    }).as_ref().map_err(Clone::clone)?;
    sender.try_send(task).map_err(|_| failed())
}
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut busy) = BUSY.lock() { *busy = busy.saturating_sub(1); }
        FREE.notify_all();
    }
}
fn permit(control: &ArchiveControl) -> Result<Permit> {
    let until = Instant::now() + Duration::from_secs(5);
    let mut busy = BUSY.lock().map_err(|_| failed())?;
    loop {
        control.check()?;
        if *busy < SLOTS { *busy += 1; return Ok(Permit); }
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(CoreError::new(ErrorCode::Busy, "archive source I/O slots occupied")); }
        busy = FREE.wait_timeout(busy, TICK.min(remaining)).map_err(|_| failed())?.0;
    }
}
fn wait<T>(receiver: &mpsc::Receiver<Result<T>>, control: &ArchiveControl) -> Result<T> {
    let until = Instant::now() + IO_DEADLINE;
    loop {
        control.check()?;
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(timeout()); }
        match receiver.recv_timeout(TICK.min(remaining)) {
            Ok(value) => { control.check()?; return value; },
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            Err(_) => return Err(failed()),
        }
    }
}
/// Metadata/listing probes must be just as cancellable as content reads.
pub(crate) fn run<T: Send + 'static>(control: &ArchiveControl,
    operation: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    let permit = permit(control)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_control = control.clone();
    dispatch(Box::new(move || {
        let _permit = permit;
        let result = worker_control.check().and_then(|_| operation());
        let _ = sender.send(result);
    }))?;
    wait(&receiver, control)
}

enum Command { Read(usize), Seek(u64) }
enum Reply { Opened(Metadata), Data(Zeroizing<Vec<u8>>), Done }
pub(crate) struct Reader {
    commands: Option<mpsc::SyncSender<Command>>, replies: Option<mpsc::Receiver<Result<Reply>>>,
    control: ArchiveControl, pub(crate) metadata: Metadata,
}
impl Reader {
    pub(crate) fn open(storage: Arc<dyn Storage>, path: ResourcePath, expected: Option<Metadata>,
        control: ArchiveControl) -> Result<Self> {
        let permit = permit(&control)?;
        let (commands, requests) = mpsc::sync_channel(1);
        let (sender, replies) = mpsc::sync_channel(1);
        let worker_control = control.clone();
        dispatch(Box::new(move || {
            // The slot stays held through handle cleanup, even after caller cancellation.
            let _permit = permit;
            let result = (|| -> Result<()> {
                worker_control.check()?;
                let mut reader = storage.open_read(&path)?;
                let metadata = reader.metadata()?;
                if metadata.kind != EntryKind::File || expected.as_ref().is_some_and(|e| *e != metadata) {
                    return Err(CoreError::new(ErrorCode::SourceChanged, "archive source changed before opening"));
                }
                if sender.send(Ok(Reply::Opened(metadata.clone()))).is_err() { return Ok(()); }
                for request in requests {
                    worker_control.check()?;
                    let reply = match request {
                        Command::Seek(position) => { reader.seek_to(position)?; Reply::Done },
                        Command::Read(count) => {
                            let mut bytes = Zeroizing::new(vec![0u8; count]);
                            let count = reader.read(&mut bytes).map_err(CoreError::io)?;
                            bytes.truncate(count);
                            if count == 0 && (reader.metadata()? != metadata || storage.metadata(&path)? != metadata) {
                                return Err(CoreError::new(ErrorCode::SourceChanged, "archive source changed while reading"));
                            }
                            Reply::Data(bytes)
                        },
                    };
                    worker_control.check()?;
                    if sender.send(Ok(reply)).is_err() { break; }
                }
                Ok(())
            })();
            if let Err(error) = result { let _ = sender.send(Err(error)); }
        }))?;
        let metadata = match wait(&replies, &control)? { Reply::Opened(m) => m, _ => return Err(failed()) };
        Ok(Self { commands: Some(commands), replies: Some(replies), control, metadata })
    }
    fn request(&mut self, command: Command) -> Result<Reply> {
        let result = (|| {
            self.control.check()?;
            self.commands.as_ref().ok_or_else(failed)?.try_send(command).map_err(|_| failed())?;
            wait(self.replies.as_ref().ok_or_else(failed)?, &self.control)
        })();
        if result.is_err() {
            // A late result has no recipient and is dropped/zeroized. No new
            // request can race a still-running provider call on this handle.
            self.commands.take(); self.replies.take();
        }
        result
    }
    pub(crate) fn seek_to(&mut self, position: u64) -> Result<()> {
        match self.request(Command::Seek(position))? { Reply::Done => Ok(()), _ => Err(failed()) }
    }
    /// Up to `count` bytes (at most 1 MiB) at the current offset; empty at EOF.
    /// Keeps the provider's error code (Unsupported: no random access).
    pub(crate) fn read_chunk(&mut self, count: usize) -> Result<Zeroizing<Vec<u8>>> {
        match self.request(Command::Read(count.min(1 << 20)))? { Reply::Data(bytes) => Ok(bytes), _ => Err(failed()) }
    }
    /// Archive libraries jump around the file. Pipe-backed provider streams
    /// (some cloud/remote documents) cannot; they fail here with Unsupported
    /// instead of deep inside a codec (У-61). Reads the last byte, then
    /// returns to the start. After an error this reader is unusable.
    pub(crate) fn probe_random_access(&mut self) -> Result<()> {
        let length = self.metadata.bytes;
        if length < 2 { return Ok(()); }
        self.seek_to(length - 1)?;
        if self.read_chunk(1)?.len() != 1 { return Err(failed()); }
        self.seek_to(0)
    }
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() { return Ok(0); }
        let bytes = self.read_chunk(output.len()).map_err(|e| io::Error::other(e.to_string()))?;
        if bytes.len() > output.len() { return Err(io::Error::other("invalid source I/O reply")); }
        output[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use std::sync::atomic::Ordering;
    #[test]
    fn cancellation_returns_before_the_blocked_probe_finishes() {
        let control = ArchiveControl::new();
        let worker_control = control.clone();
        let (entered, started) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let (returned, result) = mpsc::sync_channel(1);
        let caller = std::thread::spawn(move || {
            let value = run(&worker_control, move || { entered.send(()).unwrap(); blocked.recv().unwrap(); Ok(()) });
            returned.send(value).unwrap();
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        control.cancelled.store(true, Ordering::Release);
        // No release has been sent: the provider call is still blocked.
        let value = result.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        assert_eq!(value.unwrap().unwrap_err().code, ErrorCode::Cancelled);
        caller.join().unwrap();
    }
}
