//! Random-access reading of one Cryptomator file (E4.7: viewers, media,
//! archives inside the vault without plaintext files). Only the chunks
//! needed are read and authenticated. Format limitation (as for export):
//! Cryptomator does not authenticate the number of chunks, so removal of
//! complete trailing chunks is not detectable; everything read is authenticated.
use crate::cryptomator_ctrmac::{CtrMacChunks, CTRMAC_HEADER, CTRMAC_OVERHEAD};
use crate::cryptomator_gcm::{GcmChunks, GCM_HEADER, GCM_OVERHEAD};
use paranoic_core::{Metadata, ReadHandle};
use paranoic_crypto::{CryptoError, Result};
use zeroize::Zeroizing;

const CHUNK: u64 = 32 * 1024;

pub(crate) enum Chunks { Gcm(Box<GcmChunks>), CtrMac(CtrMacChunks) }
impl Chunks {
    fn header_len(&self) -> u64 { match self { Self::Gcm(_) => GCM_HEADER as u64, Self::CtrMac(_) => CTRMAC_HEADER as u64 } }
    fn overhead(&self) -> u64 { match self { Self::Gcm(_) => GCM_OVERHEAD as u64, Self::CtrMac(_) => CTRMAC_OVERHEAD as u64 } }
    fn open(&self, index: u64, sealed: &mut [u8]) -> Result<std::ops::Range<usize>> {
        match self { Self::Gcm(c) => c.open(index, sealed), Self::CtrMac(c) => c.open(index, sealed) }
    }
}
pub(crate) fn header_len_for(combo: crate::CryptomatorCipherCombo) -> usize {
    match combo { crate::CryptomatorCipherCombo::SivGcm => GCM_HEADER, crate::CryptomatorCipherCombo::SivCtrMac => CTRMAC_HEADER }
}

/// An opened Cryptomator file. `size()`: plaintext length derived from the
/// ciphertext length (see module note). The object must not change while open.
pub struct CryptomatorFileReader { handle: Box<dyn ReadHandle>, before: Metadata, chunks: Chunks, size: u64 }

impl CryptomatorFileReader {
    pub(crate) fn new(handle: Box<dyn ReadHandle>, before: Metadata, chunks: Chunks) -> Result<Self> {
        let (header, overhead, sealed) = (chunks.header_len(), chunks.overhead(), CHUNK + chunks.overhead());
        let body = before.bytes.checked_sub(header).ok_or(CryptoError::Authentication)?;
        let (full, rest) = (body / sealed, body % sealed);
        if rest != 0 && rest < overhead { return Err(CryptoError::Authentication); }
        let size = full * CHUNK + if rest == 0 { 0 } else { rest - overhead };
        Ok(Self { handle, before, chunks, size })
    }
    pub fn size(&self) -> u64 { self.size }
    /// Up to `len` plaintext bytes from `offset` (fewer at the end).
    pub fn read_at(&mut self, offset: u64, len: usize) -> Result<Zeroizing<Vec<u8>>> {
        let mut out = Zeroizing::new(Vec::new());
        if offset >= self.size || len == 0 { return Ok(out); }
        let end = offset.saturating_add(len as u64).min(self.size);
        let sealed_chunk = CHUNK + self.chunks.overhead();
        for index in offset / CHUNK..=(end - 1) / CHUNK {
            let start = self.chunks.header_len() + index * sealed_chunk;
            let sealed_len = sealed_chunk.min(self.before.bytes - start) as usize;
            self.handle.seek_to(start).map_err(|_| CryptoError::Io)?;
            let mut sealed = Zeroizing::new(vec![0u8; sealed_len]);
            std::io::Read::read_exact(&mut self.handle, &mut sealed).map_err(|_| CryptoError::Authentication)?;
            let plain = self.chunks.open(index, &mut sealed)?;
            let chunk_start = index * CHUNK;
            let from = offset.saturating_sub(chunk_start) as usize;
            let to = ((end - chunk_start) as usize).min(plain.len());
            out.extend_from_slice(&sealed[plain.start + from.min(to)..plain.start + to]);
        }
        if self.handle.metadata().map_err(|_| CryptoError::Io)? != self.before { return Err(CryptoError::Io); }
        Ok(out)
    }
}

impl crate::CryptomatorReader {
    /// Open a file of the vault for random access (components: decrypted names).
    pub fn open_file_reader(&self, components: &[String]) -> Result<CryptomatorFileReader> {
        let path = self.resolve_file_path(components, &mut || { self.session.id()?; Ok(()) })?;
        let mut handle = self.storage.open_read(&path).map_err(|_| CryptoError::Io)?;
        let before = handle.metadata().map_err(|_| CryptoError::Io)?;
        if before.kind != paranoic_core::EntryKind::File { return Err(CryptoError::InvalidInput); }
        let mut header = Zeroizing::new(vec![0u8; header_len_for(self.session.cipher_combo()?)]);
        std::io::Read::read_exact(&mut handle, &mut header).map_err(|_| CryptoError::Authentication)?;
        let chunks = self.session.open_chunks(&header)?;
        CryptomatorFileReader::new(handle, before, chunks)
    }
    /// Storage metadata of a file's content object (cheap freshness check).
    pub fn content_metadata(&self, components: &[String]) -> Result<Metadata> {
        let path = self.resolve_file_path(components, &mut || { self.session.id()?; Ok(()) })?;
        self.storage.metadata(&path).map_err(|_| CryptoError::Io)
    }
    /// Strong version of a file: its content location, storage metadata and
    /// BLAKE3 of the ciphertext (each write has a fresh header and file key).
    pub fn content_version(&self, components: &[String]) -> Result<(paranoic_core::Location, Metadata, [u8; 32])> {
        let path = self.resolve_file_path(components, &mut || { self.session.id()?; Ok(()) })?;
        let mut handle = self.storage.open_read(&path).map_err(|_| CryptoError::Io)?;
        let before = handle.metadata().map_err(|_| CryptoError::Io)?;
        if before.kind != paranoic_core::EntryKind::File { return Err(CryptoError::InvalidInput); }
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 16];
        loop {
            let n = std::io::Read::read(&mut handle, &mut buffer).map_err(|_| CryptoError::Io)?;
            if n == 0 { break; }
            hasher.update(&buffer[..n]);
        }
        if handle.metadata().map_err(|_| CryptoError::Io)? != before
            || self.storage.metadata(&path).map_err(|_| CryptoError::Io)? != before { return Err(CryptoError::Io); }
        Ok((paranoic_core::Location { storage: self.storage.id(), path }, before, *hasher.finalize().as_bytes()))
    }
    /// Replace an existing file's content with a prepared ciphertext spool
    /// (`prepare_file_import` of the same name) — only over the exact
    /// ciphertext `expected_blake3`, rechecked by Core when the job runs.
    pub fn submit_file_replace(&self, core: &paranoic_core::Core, prepared: &crate::CryptomatorPreparedImport,
        ciphertext_source: paranoic_core::Location, expected: (paranoic_core::Location, Metadata, [u8; 32]),
        job_id: paranoic_core::JobId) -> paranoic_core::Result<paranoic_core::JobRecord> {
        use paranoic_core::{CoreError, ErrorCode, Operation};
        let id = self.session.id().map_err(|_| CoreError::new(ErrorCode::PermissionDenied, "vault session locked"))?;
        let (location, metadata, digest) = expected;
        if id != prepared.vault_id() || location.storage != self.storage.id() || &location.path != prepared.content_path() {
            return Err(CoreError::new(ErrorCode::InvalidInput, "replacement belongs to another file"));
        }
        core.submit_with_id(job_id, Operation::ReplaceContentExpected { source: ciphertext_source, target: location,
            bytes: prepared.ciphertext_bytes(), blake3: *prepared.ciphertext_blake3(), expected_target: metadata,
            expected_target_blake3: digest })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use paranoic_core::EntryKind;
    use std::{cell::RefCell, io::{Cursor, Read, Write}, rc::Rc};

    struct Mem(Cursor<Vec<u8>>);
    impl Read for Mem { fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> { self.0.read(b) } }
    impl ReadHandle for Mem {
        fn metadata(&self) -> paranoic_core::Result<Metadata> {
            Ok(Metadata { kind: EntryKind::File, bytes: self.0.get_ref().len() as u64, modified_unix_ms: Some(1) })
        }
        fn seek_to(&mut self, offset: u64) -> paranoic_core::Result<()> { self.0.set_position(offset); Ok(()) }
    }
    struct Out(Rc<RefCell<Vec<u8>>>);
    impl Write for Out {
        fn write(&mut self, d: &[u8]) -> std::io::Result<usize> { self.0.borrow_mut().extend_from_slice(d); Ok(d.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    impl paranoic_crypto::StreamSink for Out {
        fn commit(self: Box<Self>) -> Result<()> { Ok(()) }
        fn abort(self: Box<Self>) -> Result<()> { Ok(()) }
    }
    fn reader(cipher: &[u8], header: usize, open: impl Fn(&[u8]) -> Result<Chunks>) -> Result<CryptomatorFileReader> {
        let handle = Mem(Cursor::new(cipher.to_vec()));
        let before = handle.metadata().unwrap();
        CryptomatorFileReader::new(Box::new(handle), before, open(&cipher[..header])?)
    }
    fn check(cipher: Vec<u8>, data: &[u8], header: usize, open: impl Fn(&[u8]) -> Result<Chunks> + Copy) {
        let mut r = reader(&cipher, header, open).unwrap();
        assert_eq!(r.size(), data.len() as u64);
        for (offset, len) in [(0usize, 10usize), (32_760, 20), (32_768, 32_768), (65_530, 100_000), (data.len().saturating_sub(5), 50), (0, 1 << 20)] {
            let end = (offset + len).min(data.len());
            let got = r.read_at(offset as u64, len).unwrap();
            assert_eq!(&got[..], if offset < data.len() { &data[offset..end] } else { &[][..] }, "offset {offset}");
        }
        if data.len() > 40_000 {
            // A flipped byte in the second chunk fails only reads that need it.
            let mut bad = cipher.clone(); bad[header + 32_768 + 200] ^= 1;
            let mut r = reader(&bad, header, open).unwrap();
            assert_eq!(&r.read_at(0, 100).unwrap()[..], &data[..100]);
            assert!(r.read_at(32_770, 10).is_err());
        }
    }
    #[test] fn random_reads_match_both_cipher_combos() {
        let mut master = [0u8; 64];
        for (i, b) in master.iter_mut().enumerate() { *b = (i as u8).wrapping_mul(7).wrapping_add(3); }
        for len in [0usize, 1, 32_768, 32_769, 100_000] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let gcm = crate::CryptomatorGcmKey::from_unwrapped_encryption_key(Zeroizing::new(master[..32].try_into().unwrap()));
            let out = Rc::new(RefCell::new(Vec::new()));
            gcm.encrypt(&data[..], Box::new(Out(out.clone())), Default::default(), |_| Ok(())).unwrap();
            let cipher = out.borrow().clone();
            check(cipher, &data, GCM_HEADER, |h| Ok(Chunks::Gcm(Box::new(gcm.open_header(h)?))));
            let ctr = crate::CryptomatorCtrMacKey::from_unwrapped_master_key(&master);
            let out = Rc::new(RefCell::new(Vec::new()));
            ctr.encrypt(&data[..], Box::new(Out(out.clone())), Default::default(), |_| Ok(())).unwrap();
            let cipher = out.borrow().clone();
            check(cipher, &data, CTRMAC_HEADER, |h| Ok(Chunks::CtrMac(ctr.open_header(h)?)));
        }
        // A forged header is refused before any read.
        let gcm = crate::CryptomatorGcmKey::from_unwrapped_encryption_key(Zeroizing::new([9u8; 32]));
        assert!(gcm.open_header(&[0u8; GCM_HEADER]).is_err());
    }
}
