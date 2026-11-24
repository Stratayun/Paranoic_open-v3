//! Streaming transfers between Core grants and a provider, with SHA-256
//! evidence. Sources are never deleted here. Queue/journal integration of
//! these transfers (roadmap 3.7) is separate.
use std::{io::{Read, Write}, sync::mpsc};
use paranoic_core::{Core, EntryKind, JobId, Location, ResourcePath, Storage, StorageDomain};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CloudEntry, CloudId, CloudKind, CloudProvider, CoreError, ErrorCode, Result};

/// Read until `buffer` is full or EOF; returns bytes read.
pub(crate) fn fill(source: &mut dyn Read, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(CoreError::io(error)),
        }
    }
    Ok(filled)
}

struct Hashing<'a> { inner: &'a mut dyn Read, hash: Sha256 }
impl Read for Hashing<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?; self.hash.update(&buffer[..n]); Ok(n)
    }
}

/// Upload a file from a Core grant as a NEW cloud file, then prove the stored
/// bytes: server SHA-256 when available, otherwise a full download readback.
/// Private Vault grants are refused (Vault is isolated from Cloud).
pub fn upload_from_core(core: &Core, source: &Location, provider: &dyn CloudProvider, parent: &CloudId, name: &str,
    mut progress: impl FnMut(u64) -> Result<()>) -> Result<CloudEntry> {
    let mut input = core.open_read(source)?;
    let before = input.metadata()?;
    if before.kind != EntryKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "only files can be uploaded")); }
    let mut hashing = Hashing { inner: &mut input, hash: Sha256::new() };
    let entry = provider.upload_new(parent, name, before.bytes, &mut hashing, &mut progress)?;
    let digest: [u8; 32] = hashing.hash.finalize().into();
    if input.metadata()? != before {
        return Err(CoreError::new(ErrorCode::NeedsAttention, "source changed during upload; cloud copy may differ"));
    }
    verify_remote(provider, &entry, before.bytes, &digest, &mut progress)?;
    Ok(entry)
}

/// Compare a cloud file with expected length and SHA-256 (download readback
/// if the provider did not report a SHA-256).
pub fn verify_remote(provider: &dyn CloudProvider, entry: &CloudEntry, bytes: u64, sha256: &[u8; 32],
    progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<()> {
    let mismatch = || CoreError::new(ErrorCode::Integrity, "cloud file differs from the expected content");
    if entry.kind != CloudKind::File || entry.bytes != Some(bytes) { return Err(mismatch()); }
    if let Some(server) = entry.sha256 { return if &server == sha256 { Ok(()) } else { Err(mismatch()) }; }
    let mut input = provider.open_download(&entry.id, 0)?;
    let mut hash = Sha256::new(); let mut count = 0u64;
    let mut buffer = Zeroizing::new(vec![0u8; 1 << 16]);
    loop {
        progress(count)?;
        let n = input.read(&mut buffer).map_err(CoreError::io)?;
        if n == 0 { break; }
        count += n as u64;
        if count > bytes { return Err(mismatch()); }
        hash.update(&buffer[..n]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    if count == bytes && &digest == sha256 { Ok(()) } else { Err(mismatch()) }
}

/// Download a cloud file into a NEW path of a Files grant (staged, published
/// only after the exact length was received). Returns the SHA-256.
pub fn download_to_storage(provider: &dyn CloudProvider, entry: &CloudEntry, storage: &dyn Storage, target: &ResourcePath,
    mut progress: impl FnMut(u64) -> Result<()>) -> Result<[u8; 32]> {
    if storage.domain() != StorageDomain::Files || !storage.capabilities().write || target.is_root() {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "download destination unavailable"));
    }
    let bytes = match (entry.kind, entry.bytes) { (CloudKind::File, Some(b)) => b, _ => return Err(CoreError::new(ErrorCode::Unsupported, "not a downloadable file")) };
    let current = provider.metadata(&entry.id)?;
    if current.revision != entry.revision || current.bytes != entry.bytes { return Err(crate::stale()); }
    let mut input = provider.open_download(&entry.id, 0)?;
    let mut output = storage.stage(target, JobId(uuid::Uuid::new_v4()))?;
    let mut hash = Sha256::new();
    let result = (|| {
        let mut count = 0u64; let mut buffer = Zeroizing::new(vec![0u8; 1 << 16]);
        loop {
            progress(count)?;
            let n = input.read(&mut buffer).map_err(CoreError::io)?;
            if n == 0 { break; }
            count += n as u64;
            if count > bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "cloud file grew during download")); }
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n]).map_err(CoreError::io)?;
        }
        if count != bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "cloud file shorter than listed")); }
        output.flush().map_err(CoreError::io)
    })();
    if let Err(error) = result { output.abort()?; return Err(error); }
    let digest: [u8; 32] = hash.finalize().into();
    if entry.sha256.is_some_and(|s| s != digest) { output.abort()?; return Err(CoreError::new(ErrorCode::Integrity, "downloaded bytes do not match the provider hash")); }
    output.publish()?;
    Ok(digest)
}

/// Body reader fed chunk by chunk from another thread (streaming uploads of
/// a borrowed source). Err(()) aborts the request; a closed channel is EOF.
pub(crate) struct ChannelReader { pub(crate) rx: mpsc::Receiver<std::result::Result<Zeroizing<Vec<u8>>, ()>>, pub(crate) current: Zeroizing<Vec<u8>>, pub(crate) position: usize }
impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.position == self.current.len() {
            match self.rx.recv() {
                Ok(Ok(chunk)) => { self.current = chunk; self.position = 0; }
                Ok(Err(())) => return Err(std::io::Error::other("upload cancelled")),
                Err(_) => return Ok(0),
            }
        }
        let n = buffer.len().min(self.current.len() - self.position);
        buffer[..n].copy_from_slice(&self.current[self.position..self.position + n]);
        self.position += n; Ok(n)
    }
}


/// Stream exactly `bytes` from a borrowed `source` as an HTTP body. `send`
/// runs on a scoped thread with the body; chunks are hashed via `observe`.
/// The source must end exactly at `bytes` (SourceChanged otherwise).
pub(crate) fn streamed<F>(bytes: u64, source: &mut dyn Read, progress: &mut dyn FnMut(u64) -> Result<()>,
    observe: &mut dyn FnMut(&[u8]), send: F) -> Result<reqwest::blocking::Response>
where F: FnOnce(reqwest::blocking::Body) -> Result<reqwest::blocking::Response> + Send {
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::sync_channel::<std::result::Result<Zeroizing<Vec<u8>>, ()>>(4);
        let body = reqwest::blocking::Body::sized(ChannelReader { rx, current: Zeroizing::new(Vec::new()), position: 0 }, bytes);
        let worker = scope.spawn(move || send(body));
        let mut sent = 0u64;
        let pumped: Result<()> = (|| {
            while sent < bytes {
                progress(sent)?;
                let mut chunk = Zeroizing::new(vec![0u8; (bytes - sent).min(1 << 20) as usize]);
                let n = fill(source, &mut chunk)?;
                if n < chunk.len() { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source shrank")); }
                observe(&chunk[..]); sent += n as u64;
                if tx.send(Ok(chunk)).is_err() { break; }
            }
            let mut tail = [0u8; 1];
            if source.read(&mut tail).map_err(CoreError::io)? != 0 { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source grew")); }
            Ok(())
        })();
        if pumped.is_err() { let _ = tx.send(Err(())); }
        drop(tx);
        let response = worker.join().map_err(|_| CoreError::new(ErrorCode::Internal, "upload worker failed"))?;
        pumped?;
        if sent != bytes { return Err(crate::uncertain()); }
        response
    })
}
