//! File-content codec for Cryptomator SIV_GCM (cryptolib v2).
//! This is not a vault opener: the caller must authenticate vault configuration
//! and unwrap its encryption master key before using this codec.
use std::io::{self, Read, Write};
use aes_gcm::{Aes256Gcm, Nonce, Tag, aead::{AeadInPlace, KeyInit}};
use paranoic_crypto::{CryptoError, Result, StreamSink};
use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, Zeroizing};

const CHUNK: usize = 32 * 1024;
const NONCE: usize = 12;
const TAG: usize = 16;
const HEADER: usize = 68;
const MAX_BYTES: u64 = 1 << 40;

/// Encryption half of an already-unwrapped Cryptomator master key.
/// No Debug, Clone, serialization or secret export is provided.
pub struct CryptomatorGcmKey(Zeroizing<[u8; 32]>);

#[derive(Clone, Copy)]
pub struct CryptomatorContentLimits {
    pub max_plaintext_bytes: u64,
}

impl Default for CryptomatorContentLimits {
    fn default() -> Self { Self { max_plaintext_bytes: MAX_BYTES } }
}

/// Cryptomator does not authenticate the number of chunks in a file.
/// A length obtained from the ciphertext's own metadata is NOT trusted.
#[derive(Clone, Copy)]
pub enum CryptomatorLengthPolicy {
    /// Exact length from an independently authenticated source.
    RequireTrusted(u64),
    /// Explicitly accept that removal of complete trailing chunks is undetectable.
    AllowUnauthenticated,
}

#[derive(Clone, Copy, Debug)]
pub struct CryptomatorContentOutcome {
    pub plaintext_bytes: u64,
    /// True only when compared to an independently trusted expected length.
    pub trusted_length_matched: bool,
}

impl CryptomatorGcmKey {
    pub fn from_unwrapped_encryption_key(key: Zeroizing<[u8; 32]>) -> Self { Self(key) }

    /// Create a new content file; never modify chunks in an existing file.
    /// Each call generates an independent file key and header nonce.
    pub fn encrypt<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, mut input: R, mut output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, mut progress: F,
    ) -> Result<u64> {
        let result = (|| {
            validate(limits)?;
            progress(0)?;
            let header_nonce = random_nonce()?;
            let mut payload = Zeroizing::new([0u8; 40]);
            payload[..8].fill(0xff); // reserved signed long -1
            OsRng.try_fill_bytes(&mut payload[8..]).map_err(|_| CryptoError::Entropy)?;
            let file_cipher = cipher(&payload[8..])?;
            let header_tag = cipher(&self.0[..])?.encrypt_in_place_detached(
                Nonce::from_slice(&header_nonce), b"", &mut payload[..],
            ).map_err(|_| CryptoError::Authentication)?;
            output.write_all(&header_nonce)?;
            output.write_all(&payload[..])?;
            output.write_all(&header_tag)?;
            let mut buffer = Zeroizing::new(vec![0u8; CHUNK]);
            let mut total = 0u64;
            let mut index = 0u64;
            loop {
                progress(total)?;
                // Read at most remaining budget plus one byte to detect excess.
                let capacity = (limits.max_plaintext_bytes - total).saturating_add(1)
                    .min(CHUNK as u64) as usize;
                let count = read_block(&mut input, &mut buffer[..capacity], &mut || progress(total))?;
                if count == 0 { break; }
                let next = total.checked_add(count as u64).ok_or(CryptoError::LimitExceeded)?;
                if next > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                let nonce = random_nonce()?;
                let aad = chunk_aad(index, &header_nonce);
                let tag = file_cipher.encrypt_in_place_detached(
                    Nonce::from_slice(&nonce), &aad, &mut buffer[..count],
                ).map_err(|_| CryptoError::Authentication)?;
                output.write_all(&nonce)?;
                output.write_all(&buffer[..count])?;
                output.write_all(&tag)?;
                buffer[..].zeroize();
                total = next;
                index += 1; // bounded by MAX_BYTES / CHUNK
            }
            progress(total)?;
            Ok(total)
        })();
        finish(output, result)
    }

    /// Authenticate each chunk before writing it into a private provisional sink.
    /// Publication requires EOF and the selected length policy to succeed.
    pub fn decrypt<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, mut input: R, mut output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, length: CryptomatorLengthPolicy,
        mut progress: F,
    ) -> Result<CryptomatorContentOutcome> {
        let result = (|| {
            validate(limits)?;
            let expected = match length {
                CryptomatorLengthPolicy::RequireTrusted(size) => {
                    if size > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                    Some(size)
                }
                CryptomatorLengthPolicy::AllowUnauthenticated => None,
            };
            progress(0)?;
            let mut header = Zeroizing::new([0u8; HEADER]);
            if read_block(&mut input, &mut header[..], &mut || progress(0))? != HEADER {
                return Err(CryptoError::Authentication);
            }
            let mut header_nonce = [0u8; NONCE];
            header_nonce.copy_from_slice(&header[..NONCE]);
            let tag = Tag::clone_from_slice(&header[52..]);
            cipher(&self.0[..])?.decrypt_in_place_detached(
                Nonce::from_slice(&header_nonce), b"", &mut header[12..52], &tag,
            ).map_err(|_| CryptoError::Authentication)?;
            // Reserved bytes are authenticated but have no length semantics.
            let file_cipher = cipher(&header[20..52])?;
            header.zeroize();
            let mut buffer = Zeroizing::new(vec![0u8; CHUNK + NONCE + TAG]);
            let mut total = 0u64;
            let mut index = 0u64;
            loop {
                progress(total)?;
                let count = read_block(&mut input, &mut buffer[..], &mut || progress(total))?;
                if count == 0 { break; }
                if count < NONCE + TAG { return Err(CryptoError::Authentication); }
                let plain_len = count - NONCE - TAG;
                let next = total.checked_add(plain_len as u64).ok_or(CryptoError::LimitExceeded)?;
                if next > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                if expected.is_some_and(|size| next > size) { return Err(CryptoError::Authentication); }
                let mut nonce = [0u8; NONCE];
                nonce.copy_from_slice(&buffer[..NONCE]);
                let tag = Tag::clone_from_slice(&buffer[count - TAG..count]);
                let aad = chunk_aad(index, &header_nonce);
                file_cipher.decrypt_in_place_detached(
                    Nonce::from_slice(&nonce), &aad, &mut buffer[NONCE..count - TAG], &tag,
                ).map_err(|_| CryptoError::Authentication)?;
                progress(total)?;
                output.write_all(&buffer[NONCE..count - TAG])?;
                buffer[..].zeroize();
                total = next;
                index += 1;
                if count < CHUNK + NONCE + TAG { break; } // read_block observed EOF
            }
            if expected.is_some_and(|size| size != total) { return Err(CryptoError::Authentication); }
            progress(total)?;
            Ok(CryptomatorContentOutcome {
                plaintext_bytes: total, trusted_length_matched: expected.is_some(),
            })
        })();
        finish(output, result)
    }
}

pub(crate) fn validate(limits: CryptomatorContentLimits) -> Result<()> {
    if limits.max_plaintext_bytes > MAX_BYTES { return Err(CryptoError::InvalidInput); }
    Ok(())
}

fn cipher(key: &[u8]) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidInput)
}

fn random_nonce() -> Result<[u8; NONCE]> {
    let mut nonce = [0u8; NONCE];
    OsRng.try_fill_bytes(&mut nonce).map_err(|_| CryptoError::Entropy)?;
    Ok(nonce)
}

fn chunk_aad(index: u64, header_nonce: &[u8; NONCE]) -> [u8; 20] {
    let mut aad = [0u8; 20];
    aad[..8].copy_from_slice(&index.to_be_bytes());
    aad[8..].copy_from_slice(header_nonce);
    aad
}

pub(crate) fn read_block<R: Read, F: FnMut() -> Result<()>>(
    input: &mut R, buffer: &mut [u8], checkpoint: &mut F,
) -> Result<usize> {
    let mut used = 0;
    while used < buffer.len() {
        checkpoint()?;
        match input.read(&mut buffer[used..]) {
            Ok(0) => break,
            Ok(count) => used += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(used)
}

pub(crate) fn finish<T>(output: Box<dyn StreamSink>, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => { output.commit()?; Ok(value) }
        Err(error) => {
            output.abort().map_err(|_| CryptoError::CleanupFailed)?;
            Err(error)
        }
    }
}

/// Per-file chunk opener (random access, E4.7): file key from the header.
pub(crate) struct GcmChunks { cipher: Aes256Gcm, header_nonce: [u8; NONCE] }
pub(crate) const GCM_HEADER: usize = HEADER;
pub(crate) const GCM_OVERHEAD: usize = NONCE + TAG;
impl CryptomatorGcmKey {
    pub(crate) fn open_header(&self, header: &[u8]) -> Result<GcmChunks> {
        if header.len() != HEADER { return Err(CryptoError::Authentication); }
        let mut copy = Zeroizing::new([0u8; HEADER]);
        copy.copy_from_slice(header);
        let mut header_nonce = [0u8; NONCE];
        header_nonce.copy_from_slice(&copy[..NONCE]);
        let tag = Tag::clone_from_slice(&copy[52..]);
        cipher(&self.0[..])?.decrypt_in_place_detached(Nonce::from_slice(&header_nonce), b"", &mut copy[12..52], &tag)
            .map_err(|_| CryptoError::Authentication)?;
        Ok(GcmChunks { cipher: cipher(&copy[20..52])?, header_nonce })
    }
}
impl GcmChunks {
    /// Authenticate and decrypt sealed chunk `index` in place; plaintext range.
    pub(crate) fn open(&self, index: u64, sealed: &mut [u8]) -> Result<std::ops::Range<usize>> {
        if sealed.len() < NONCE + TAG || sealed.len() > CHUNK + NONCE + TAG { return Err(CryptoError::Authentication); }
        let end = sealed.len() - TAG;
        let mut nonce = [0u8; NONCE];
        nonce.copy_from_slice(&sealed[..NONCE]);
        let tag = Tag::clone_from_slice(&sealed[end..]);
        self.cipher.decrypt_in_place_detached(Nonce::from_slice(&nonce), &chunk_aad(index, &self.header_nonce), &mut sealed[NONCE..end], &tag)
            .map_err(|_| CryptoError::Authentication)?;
        Ok(NONCE..end)
    }
}
