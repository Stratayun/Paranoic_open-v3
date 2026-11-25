//! 7z extraction in a separate, confined OS process (register У-5).
//!
//! The 7z decoders (LZMA/LZMA2/BCJ/AES, header decoding) run only in the worker
//! (`paranoic-pdf-worker --sevenz`, the same confined binary as PDF: seccomp
//! and rlimits on Linux/Android, AppContainer + job on Windows, isolated UID
//! on Android). The
//! worker has no file access: it reads the archive by asking the application
//! for byte ranges over its stdin/stdout, and streams back the index and the
//! decoded entries. The application re-validates the index (names, sizes,
//! limits), enforces every entry's declared size while writing to its private
//! staging, and publishes only a complete, verified extraction. A crash, a
//! hang (no message within the deadline) or any protocol violation kills the
//! worker and discards the staging.
//!
//! Protocol (all integers little endian).
//! App → worker: config `u64 length, u32 entries, u64 entry_bytes, u64 total_bytes,
//! u64 directory_bytes, u32 depth, u32 password_len, password`; read reply
//! `u32 n, n bytes`; command `u8` (1 extract, 0 quit).
//! Worker → app: `u8 tag, u32 len, payload`: 1 READ(u64 offset, u32 len),
//! 2 INDEX(entries), 3 BEGIN(u32 index), 4 DATA(bytes ≤ 64 KiB), 5 END,
//! 6 DONE, 7 ERROR(u8: 1 invalid, 2 limit, 3 password required, 4 wrong password).
//! INDEX entry: `u8 flags (bit 0 folder, bit 1 encrypted), u64 bytes, u64 compressed, u32 name_len, name`.
use std::{cell::RefCell, io::{Read, Seek, SeekFrom, Write}, path::Path, rc::Rc, sync::{mpsc, Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use crate::worker_process::{WorkerHost, WorkerProcess};
use paranoic_core::{ResourcePath, Storage};
use zeroize::Zeroizing;
use crate::{ArchiveEntry, ArchiveLimits, CodecLimits, CoreError, ErrorCode, PreparedExtraction, Result, SevenZipService};

const READ_MAX: u32 = 1 << 20;
const DATA_MAX: usize = 64 * 1024;
const MESSAGE_MAX: u32 = 64 << 20;
/// No message from the worker for this long = hung; it is killed.
const DEADLINE: Duration = Duration::from_secs(60);

fn unavailable() -> CoreError { CoreError::new(ErrorCode::Unsupported, "archive worker is unavailable") }
fn protocol() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "archive worker protocol violation") }
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "archive codec or layout rejected") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "archive codec limit exceeded") }
/// Worker error code → error (the worker cannot send messages, only codes).
fn worker_error(code: u8) -> CoreError {
    match code { 2 => large(), 3 => crate::archive::password_required(), 4 => crate::archive::wrong_password(), _ => invalid() }
}
fn error_code(error: &CoreError) -> u8 {
    if error.code == ErrorCode::LimitExceeded { 2 }
    else if crate::is_password_required(error) { 3 }
    else if crate::is_wrong_password(error) { 4 }
    else { 1 }
}

// ------------------------------------------------------------------ worker side

struct Channel<I, O> { input: I, output: O }
impl<I: Read, O: Write> Channel<I, O> {
    fn send(&mut self, tag: u8, payload: &[u8]) -> std::io::Result<()> {
        self.output.write_all(&[tag])?;
        self.output.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.output.write_all(payload)?;
        self.output.flush()
    }
}

/// The archive as seen by the worker: ranges fetched from the application.
struct Remote<I, O> { channel: Rc<RefCell<Channel<I, O>>>, length: u64, position: u64 }
impl<I: Read, O: Write> Read for Remote<I, O> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = (buf.len() as u64).min(self.length.saturating_sub(self.position)).min(u64::from(READ_MAX)) as u32;
        if want == 0 { return Ok(0); }
        let mut channel = self.channel.borrow_mut();
        let mut request = self.position.to_le_bytes().to_vec(); request.extend_from_slice(&want.to_le_bytes());
        channel.send(1, &request)?;
        let mut n = [0u8; 4]; channel.input.read_exact(&mut n)?;
        let n = u32::from_le_bytes(n);
        if n == 0 || n > want { return Err(std::io::Error::other("bad read reply")); }
        channel.input.read_exact(&mut buf[..n as usize])?;
        self.position += u64::from(n);
        Ok(n as usize)
    }
}
impl<I, O> Seek for Remote<I, O> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.length.checked_add_signed(d),
            SeekFrom::Current(d) => self.position.checked_add_signed(d),
        }.ok_or_else(|| std::io::Error::other("seek out of range"))?;
        self.position = next;
        Ok(next)
    }
}

fn read_u32(input: &mut impl Read) -> std::io::Result<u32> { let mut b = [0u8; 4]; input.read_exact(&mut b)?; Ok(u32::from_le_bytes(b)) }
fn read_u64(input: &mut impl Read) -> std::io::Result<u64> { let mut b = [0u8; 8]; input.read_exact(&mut b)?; Ok(u64::from_le_bytes(b)) }

/// Worker main loop (call after the process confined itself). Not for app use.
#[doc(hidden)]
pub fn serve_worker<I: Read + 'static, O: Write + 'static>(mut input: I, output: O) {
    let Ok(config) = (|| -> std::io::Result<_> {
        let length = read_u64(&mut input)?;
        let entries = read_u32(&mut input)? as usize;
        let (entry_bytes, total_bytes, directory_bytes) = (read_u64(&mut input)?, read_u64(&mut input)?, read_u64(&mut input)?);
        let depth = read_u32(&mut input)? as usize;
        let password_len = read_u32(&mut input)?;
        if password_len > 4096 { return Err(std::io::Error::other("password too long")); }
        let mut password = Zeroizing::new(vec![0u8; password_len as usize]);
        input.read_exact(&mut password)?;
        Ok((length, entries, entry_bytes, total_bytes, directory_bytes, depth, password))
    })() else { return };
    let (length, entries, entry_bytes, total_bytes, directory_bytes, depth, password) = config;
    let channel = Rc::new(RefCell::new(Channel { input, output }));
    let fail = |code: u8| { let _ = channel.borrow_mut().send(7, &[code]); };
    let code = error_code;
    let limits = CodecLimits { archive: ArchiveLimits { archive_bytes: length.max(32), entries, entry_bytes, total_bytes, directory_bytes,
        ratio: ArchiveLimits::default().ratio, depth }, pending_files: CodecLimits::default().pending_files };
    let password = match std::str::from_utf8(&password) {
        Ok("") => None,
        Ok(p) => Some(Zeroizing::new(p.to_owned())),
        Err(_) => { fail(1); return; }
    };
    let remote = Remote { channel: Rc::clone(&channel), length, position: 0 };
    let mut service = match SevenZipService::open(remote, limits, password, || Ok(())) { Ok(s) => s, Err(e) => { fail(code(&e)); return; } };
    let mut index = Vec::new();
    for e in service.entries() {
        let name = e.path.as_str().as_bytes();
        index.push(u8::from(e.directory) | (u8::from(e.encrypted) << 1)); index.extend_from_slice(&e.bytes.to_le_bytes());
        index.extend_from_slice(&e.compressed_bytes.to_le_bytes()); index.extend_from_slice(&(name.len() as u32).to_le_bytes());
        index.extend_from_slice(name);
    }
    if channel.borrow_mut().send(2, &index).is_err() { return; }
    let mut command = [0u8; 1];
    if channel.borrow_mut().input.read_exact(&mut command).is_err() || command[0] != 1 { return; }
    let result = service.for_each(|position, stream| {
        let io = |_| CoreError::new(ErrorCode::Io, "archive worker pipe closed");
        channel.borrow_mut().send(3, &(position as u32).to_le_bytes()).map_err(io)?;
        let mut buffer = Zeroizing::new(vec![0u8; DATA_MAX]);
        loop {
            let n = stream.read(&mut buffer).map_err(|_| invalid())?;
            if n == 0 { break; }
            channel.borrow_mut().send(4, &buffer[..n]).map_err(io)?;
        }
        channel.borrow_mut().send(5, &[]).map_err(io)
    });
    match result { Ok(()) => { let _ = channel.borrow_mut().send(6, &[]); } Err(e) => fail(code(&e)) }
}

// ------------------------------------------------------------- application side

enum Message { Read(u64, u32), Index(Vec<u8>), Begin(u32), Data(Vec<u8>), End, Done, Error(u8) }

struct Session<R> { child: WorkerProcess, requests: mpsc::SyncSender<Zeroizing<Vec<u8>>>, messages: mpsc::Receiver<Option<(u8, Vec<u8>)>>,
    source: R, length: u64, dead: bool, cancelled: Arc<AtomicBool>, until: Instant,
    read_requests: u64, remaining_read_bytes: u64 }
impl<R: Read + Seek> Session<R> {
    fn check(&mut self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            self.kill(); return Err(CoreError::new(ErrorCode::Cancelled, "archive worker cancelled"));
        }
        if Instant::now() >= self.until { self.kill(); return Err(large()); }
        if self.dead { return Err(unavailable()); }
        Ok(())
    }
    fn kill(&mut self) { self.dead = true; let _ = self.child.kill(); let _ = self.child.wait(); }
    fn send(&mut self, message: Zeroizing<Vec<u8>>) -> Result<()> {
        let deadline = (Instant::now() + DEADLINE).min(self.until);
        let mut pending = Some(message);
        while let Some(message) = pending.take() {
            self.check()?;
            match self.requests.try_send(message) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(m)) if std::time::Instant::now() < deadline => { std::thread::sleep(Duration::from_millis(2)); pending = Some(m); }
                Err(_) => { self.kill(); return Err(unavailable()); }
            }
        }
        Ok(())
    }
    /// Next message other than READ; READ requests are answered here.
    fn next(&mut self) -> Result<Message> {
        // READ frames cannot extend the wait for the next meaningful frame.
        let deadline = (Instant::now() + DEADLINE).min(self.until);
        loop {
            self.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { self.kill(); return Err(unavailable()); }
            let (tag, payload) = match self.messages.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(Some(m)) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                _ => { self.kill(); return Err(unavailable()); }
            };
            let message = match (tag, payload.len()) {
                (1, 12) => Message::Read(u64::from_le_bytes(payload[..8].try_into().map_err(|_| protocol())?),
                    u32::from_le_bytes(payload[8..].try_into().map_err(|_| protocol())?)),
                (2, _) => Message::Index(payload),
                (3, 4) => Message::Begin(u32::from_le_bytes(payload[..4].try_into().map_err(|_| protocol())?)),
                (4, n) if (1..=DATA_MAX).contains(&n) => Message::Data(payload),
                (5, 0) => Message::End,
                (6, 0) => Message::Done,
                (7, 1) => Message::Error(payload[0]),
                _ => { self.kill(); return Err(protocol()); }
            };
            let Message::Read(offset, len) = message else { return Ok(message) };
            if len == 0 || len > READ_MAX || offset.checked_add(u64::from(len)).is_none_or(|end| end > self.length) { self.kill(); return Err(protocol()); }
            if self.read_requests >= 1_000_000 || u64::from(len) > self.remaining_read_bytes {
                self.kill(); return Err(large());
            }
            self.read_requests += 1;
            self.remaining_read_bytes -= u64::from(len);
            let mut reply = Zeroizing::new(vec![0u8; 4 + len as usize]);
            self.source.seek(SeekFrom::Start(offset)).map_err(CoreError::io)?;
            self.source.read_exact(&mut reply[4..]).map_err(CoreError::io)?;
            self.check()?;
            reply[..4].copy_from_slice(&len.to_le_bytes());
            self.send(reply)?;
        }
    }
}
impl<R> Drop for Session<R> { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); } }

/// One decoded entry as a stream of DATA messages up to END.
struct EntryStream<'s, R> { session: &'s mut Session<R>, buffer: Vec<u8>, at: usize, ended: bool, failure: Option<CoreError> }
impl<R: Read + Seek> Read for EntryStream<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.buffer.len() {
            if self.ended { return Ok(0); }
            match self.session.next() {
                Ok(Message::Data(d)) => { self.buffer = d; self.at = 0; }
                Ok(Message::End) => { self.ended = true; return Ok(0); }
                Ok(Message::Error(c)) => { self.failure = Some(worker_error(c)); return Err(std::io::Error::other("worker error")); }
                Ok(_) => { self.session.kill(); self.failure = Some(protocol()); return Err(std::io::Error::other("protocol")); }
                Err(e) => { self.failure = Some(e); return Err(std::io::Error::other("worker")); }
            }
        }
        let n = out.len().min(self.buffer.len() - self.at);
        out[..n].copy_from_slice(&self.buffer[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// 7z archive opened through the confined worker. Same limits and results as
/// `SevenZipService`, but no decoder runs in the application process.
pub struct IsolatedSevenZip<R: Read + Seek> { session: Session<R>, entries: Vec<ArchiveEntry>, limits: CodecLimits }

impl<R: Read + Seek> IsolatedSevenZip<R> {
    /// `worker`: absolute path of `paranoic-pdf-worker` shipped with the app.
    pub fn open(source: R, limits: CodecLimits, password: Option<Zeroizing<String>>, worker: &Path,
        check: impl FnMut() -> Result<()>) -> Result<Self> {
        Self::open_cancellable(source, limits, password, worker, Arc::new(AtomicBool::new(false)), check)
    }
    pub fn open_cancellable(source: R, limits: CodecLimits, password: Option<Zeroizing<String>>, worker: &Path,
        cancelled: Arc<AtomicBool>, check: impl FnMut() -> Result<()>) -> Result<Self> {
        Self::open_with_host(source, limits, password, worker, cancelled, None, check)
    }
    pub fn open_with_host(mut source: R, limits: CodecLimits, password: Option<Zeroizing<String>>, worker: &Path,
        cancelled: Arc<AtomicBool>, host: Option<Arc<dyn WorkerHost>>, mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        limits.archive.validate()?; check()?;
        if cancelled.load(Ordering::Acquire) { return Err(CoreError::new(ErrorCode::Cancelled, "archive worker cancelled")); }
        if !worker.is_absolute() { return Err(unavailable()); }
        let length = source.seek(SeekFrom::End(0)).map_err(CoreError::io)?;
        if length > limits.archive.archive_bytes || length < 32 { return Err(large()); }
        if password.as_ref().is_some_and(|p| p.len() > 4096) { return Err(invalid()); }
        let mut child = WorkerProcess::spawn(worker, None, host)?;
        let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else { let _ = child.kill(); let _ = child.wait(); return Err(unavailable()); };
        let (sender, messages) = mpsc::sync_channel(1);
        let (requests, outgoing) = mpsc::sync_channel::<Zeroizing<Vec<u8>>>(1);
        std::thread::Builder::new().name("paranoic-7z-request".into()).spawn(move || {
            for message in outgoing { if stdin.write_all(&message).and_then(|_| stdin.flush()).is_err() { return; } }
        }).map_err(|_| { let _ = child.kill(); unavailable() })?;
        std::thread::Builder::new().name("paranoic-7z-reply".into()).spawn(move || loop {
            let mut head = [0u8; 5];
            if stdout.read_exact(&mut head).is_err() { let _ = sender.send(None); return; }
            let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
            if len > MESSAGE_MAX { let _ = sender.send(None); return; }
            let mut payload = vec![0u8; len as usize];
            if stdout.read_exact(&mut payload).is_err() || sender.send(Some((head[0], payload))).is_err() { return; }
        }).map_err(|_| { let _ = child.kill(); unavailable() })?;
        let mut session = Session { child, requests, messages, source, length, dead: false, cancelled,
            until: Instant::now() + Duration::from_secs(1800), read_requests: 0,
            remaining_read_bytes: length.saturating_mul(8).saturating_add(64 << 20) };
        let a = limits.archive;
        let mut config = Zeroizing::new(Vec::new());
        config.extend_from_slice(&length.to_le_bytes());
        config.extend_from_slice(&(a.entries as u32).to_le_bytes());
        for v in [a.entry_bytes, a.total_bytes, a.directory_bytes] { config.extend_from_slice(&v.to_le_bytes()); }
        config.extend_from_slice(&(a.depth as u32).to_le_bytes());
        let secret = password.as_ref().map(|p| p.as_bytes()).unwrap_or_default();
        config.extend_from_slice(&(secret.len() as u32).to_le_bytes()); config.extend_from_slice(secret);
        session.send(config)?;
        check()?;
        let raw = match session.next()? {
            Message::Index(raw) => raw,
            Message::Error(c) => return Err(worker_error(c)),
            _ => { session.kill(); return Err(protocol()); }
        };
        // Re-validate in the application: names, kinds, sizes and totals.
        let mut entries = Vec::new();
        let mut at = 0usize;
        while at < raw.len() {
            if entries.len() >= limits.archive.entries || raw.len() - at < 21 { session.kill(); return Err(protocol()); }
            let flags = raw[at];
            if flags & !0b11 != 0 || flags == 0b11 { session.kill(); return Err(protocol()); }
            let (directory, encrypted) = (flags & 1 != 0, flags & 2 != 0);
            let field = |i: usize| raw.get(i..i + 8).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
            let (Some(bytes), Some(compressed)) = (field(at + 1), field(at + 9)) else { session.kill(); return Err(protocol()) };
            let name_len = u32::from_le_bytes(raw[at + 17..at + 21].try_into().map_err(|_| protocol())?) as usize;
            let Some(name) = raw.get(at + 21..at + 21 + name_len) else { session.kill(); return Err(protocol()) };
            let name = std::str::from_utf8(name).map_err(|_| invalid())?;
            entries.push(ArchiveEntry { path: ResourcePath::parse(name)?, directory, bytes, compressed_bytes: compressed, encrypted });
            at += 21 + name_len;
            check()?;
        }
        let entries = crate::other_archives::index(entries, limits)?;
        Ok(Self { session, entries, limits })
    }
    pub fn entries(&self) -> &[ArchiveEntry] { &self.entries }

    /// Phase 1 for 7z: decode in the worker, stage in the application.
    /// `spool_root` must not exist on the private spool grant.
    /// Plan 4.7: every file entry as a stream of exactly its declared size,
    /// decoded in the confined worker — nothing is written here (the caller
    /// encrypts straight into a vault). Folders are skipped. A password
    /// problem or damage stops the walk with the same errors as extraction.
    pub fn for_each_file(mut self, mut f: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<()>) -> Result<()> {
        let outcome = (|| -> Result<()> {
            self.session.send(Zeroizing::new(vec![1u8]))?;
            let mut seen = vec![false; self.entries.len()];
            loop {
                match self.session.next()? {
                    Message::Begin(index) => {
                        let index = index as usize;
                        if seen.get(index).copied() != Some(false) { self.session.kill(); return Err(protocol()); }
                        seen[index] = true;
                        let entry = self.entries[index].clone();
                        let mut stream = EntryStream { session: &mut self.session, buffer: Vec::new(), at: 0, ended: false, failure: None };
                        // The worker sends END only after the decoder verified the entry's
                        // CRC/AES at its end. An empty entry waits for that END before the
                        // callback can publish it (audit 6 for 7z, U-76).
                        if !entry.directory && entry.bytes == 0 {
                            let mut probe = [0u8; 1];
                            match stream.read(&mut probe) {
                                Ok(0) => {}
                                Ok(_) => { self.session.kill(); return Err(invalid()); }
                                Err(_) => return Err(stream.failure.take().unwrap_or_else(protocol)),
                            }
                        }
                        let given = if entry.directory { Ok(()) } else {
                            let mut exact = crate::archive::ExactRead { inner: &mut stream, left: entry.bytes };
                            let result = f(&entry, &mut exact);
                            if result.is_ok() && exact.left != 0 { Err(invalid()) } else { result }
                        };
                        if let Some(failure) = stream.failure.take() { return Err(failure); }
                        given?;
                        // Nothing may follow the declared size: END must come now.
                        let mut rest = [0u8; 1];
                        if stream.read(&mut rest).map_err(|_| stream.failure.take().unwrap_or_else(protocol))? != 0 { self.session.kill(); return Err(invalid()); }
                    }
                    Message::Done if seen.iter().all(|s| *s) => return Ok(()),
                    Message::Error(c) => return Err(worker_error(c)),
                    _ => { self.session.kill(); return Err(protocol()); }
                }
            }
        })();
        self.session.kill();
        outcome
    }
    pub fn prepare_extraction(mut self, spool: &dyn Storage, spool_root: &ResourcePath,
        mut check: impl FnMut(u64) -> Result<()>) -> Result<PreparedExtraction> {
        crate::extraction::spool_ready(spool, spool_root)?;
        let mut record = crate::extraction::Recorder::default();
        let result = self.extract_into(spool, spool_root, &mut check, &mut record);
        self.session.kill();
        crate::extraction::finish(spool, spool_root, record, result)
    }
    fn extract_into(&mut self, spool: &dyn Storage, root: &ResourcePath, check: &mut impl FnMut(u64) -> Result<()>,
        record: &mut crate::extraction::Recorder) -> Result<u64> {
        check(0)?;
        let mut destination = crate::other_archives::Destination::private(spool, root, &self.entries, self.limits.pending_files, record)?;
        let outcome = (|| -> Result<()> {
            self.session.send(Zeroizing::new(vec![1u8]))?;
            let mut seen = vec![false; self.entries.len()];
            loop {
                match self.session.next()? {
                    Message::Begin(index) => {
                        let index = index as usize;
                        if seen.get(index).copied() != Some(false) { self.session.kill(); return Err(protocol()); }
                        seen[index] = true;
                        let entry = self.entries[index].clone();
                        let mut stream = EntryStream { session: &mut self.session, buffer: Vec::new(), at: 0, ended: false, failure: None };
                        let pushed = destination.push(&entry, &mut stream, check);
                        if let Some(failure) = stream.failure.take() { return Err(failure); }
                        pushed?;
                        // Directories carry no data: END must follow at once.
                        let mut rest = [0u8; 1];
                        if stream.read(&mut rest).map_err(|_| stream.failure.take().unwrap_or_else(protocol))? != 0 { self.session.kill(); return Err(protocol()); }
                    }
                    Message::Done if seen.iter().all(|s| *s) => return Ok(()),
                    Message::Error(c) => return Err(worker_error(c)),
                    _ => { self.session.kill(); return Err(protocol()); }
                }
            }
        })();
        match outcome {
            Ok(()) => destination.publish(check),
            Err(error) => { destination.abort()?; Err(error) }
        }
    }
}

