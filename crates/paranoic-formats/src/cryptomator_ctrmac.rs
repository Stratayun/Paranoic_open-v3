//! File-content codec for Cryptomator SIV_CTRMAC (vault format 7, and format 8
//! vaults configured with cipherCombo SIV_CTRMAC). Layout per cryptolib v1:
//! header = nonce(16) | AES-CTR(encKey, nonce)(reserved i64 -1 | fileKey 32) |
//! HMAC-SHA256(macKey, nonce | encrypted payload); each chunk = nonce(16) |
//! AES-CTR(fileKey, nonce)(<=32 KiB) | HMAC-SHA256(macKey, headerNonce |
//! chunk index u64 BE | nonce | ciphertext). MACs are checked before decryption.
use std::io::{Read, Write};
use aes::Aes256;
use cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use paranoic_crypto::{CryptoError, Result, StreamSink};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};
use crate::cryptomator_gcm::{finish, read_block, validate};
use crate::{CryptomatorContentLimits, CryptomatorContentOutcome, CryptomatorLengthPolicy};

type Ctr = ctr::Ctr128BE<Aes256>;
const CHUNK: usize = 32 * 1024;
const NONCE: usize = 16;
const MAC: usize = 32;
const HEADER: usize = NONCE + 40 + MAC;

/// Both halves of an unwrapped master key. No Debug/Clone/serde/export.
pub struct CryptomatorCtrMacKey {
    encryption: Zeroizing<[u8; 32]>,
    mac: Zeroizing<[u8; 32]>,
}

fn random_nonce() -> Result<[u8; NONCE]> {
    let mut nonce = [0u8; NONCE];
    OsRng.try_fill_bytes(&mut nonce).map_err(|_| CryptoError::Entropy)?;
    Ok(nonce)
}

fn ctr(key: &[u8], nonce: &[u8]) -> Result<Ctr> {
    Ctr::new_from_slices(key, nonce).map_err(|_| CryptoError::InvalidInput)
}

impl CryptomatorCtrMacKey {
    pub fn from_unwrapped_master_key(master: &[u8; 64]) -> Self {
        let mut encryption = Zeroizing::new([0u8; 32]);
        let mut mac = Zeroizing::new([0u8; 32]);
        encryption.copy_from_slice(&master[..32]);
        mac.copy_from_slice(&master[32..]);
        Self { encryption, mac }
    }

    fn hmac(&self) -> Result<Hmac<Sha256>> {
        <Hmac<Sha256> as Mac>::new_from_slice(&self.mac[..]).map_err(|_| CryptoError::InvalidInput)
    }

    fn chunk_mac(&self, header_nonce: &[u8], index: u64, nonce_and_ciphertext: &[u8]) -> Result<Hmac<Sha256>> {
        let mut mac = self.hmac()?;
        mac.update(header_nonce);
        mac.update(&index.to_be_bytes());
        mac.update(nonce_and_ciphertext);
        Ok(mac)
    }

    pub fn encrypt<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, mut input: R, mut output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, mut progress: F,
    ) -> Result<u64> {
        let result = (|| {
            validate(limits)?;
            progress(0)?;
            let header_nonce = random_nonce()?;
            let mut payload = Zeroizing::new([0u8; 40]);
            payload[..8].fill(0xff);
            OsRng.try_fill_bytes(&mut payload[8..]).map_err(|_| CryptoError::Entropy)?;
            let mut file_key = Zeroizing::new([0u8; 32]);
            file_key.copy_from_slice(&payload[8..]);
            ctr(&self.encryption[..], &header_nonce)?.apply_keystream(&mut payload[..]);
            let mut mac = self.hmac()?;
            mac.update(&header_nonce);
            mac.update(&payload[..]);
            output.write_all(&header_nonce)?;
            output.write_all(&payload[..])?;
            output.write_all(&mac.finalize().into_bytes())?;
            let mut buffer = Zeroizing::new(vec![0u8; NONCE + CHUNK]);
            let mut total = 0u64;
            let mut index = 0u64;
            loop {
                progress(total)?;
                let capacity = (limits.max_plaintext_bytes - total).saturating_add(1).min(CHUNK as u64) as usize;
                let count = read_block(&mut input, &mut buffer[NONCE..NONCE + capacity], &mut || progress(total))?;
                if count == 0 { break; }
                let next = total.checked_add(count as u64).ok_or(CryptoError::LimitExceeded)?;
                if next > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                let nonce = random_nonce()?;
                buffer[..NONCE].copy_from_slice(&nonce);
                ctr(&file_key[..], &nonce)?.apply_keystream(&mut buffer[NONCE..NONCE + count]);
                let tag = self.chunk_mac(&header_nonce, index, &buffer[..NONCE + count])?.finalize().into_bytes();
                output.write_all(&buffer[..NONCE + count])?;
                output.write_all(&tag)?;
                buffer[..].zeroize();
                total = next;
                index += 1;
            }
            progress(total)?;
            Ok(total)
        })();
        finish(output, result)
    }

    pub fn decrypt<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, mut input: R, mut output: Box<dyn StreamSink>,
        limits: CryptomatorContentLimits, length: CryptomatorLengthPolicy, mut progress: F,
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
            let mut mac = self.hmac()?;
            mac.update(&header[..NONCE + 40]);
            mac.verify_slice(&header[NONCE + 40..]).map_err(|_| CryptoError::Authentication)?;
            let mut header_nonce = [0u8; NONCE];
            header_nonce.copy_from_slice(&header[..NONCE]);
            ctr(&self.encryption[..], &header_nonce)?.apply_keystream(&mut header[NONCE..NONCE + 40]);
            let mut file_key = Zeroizing::new([0u8; 32]);
            file_key.copy_from_slice(&header[NONCE + 8..NONCE + 40]);
            header.zeroize();
            let mut buffer = Zeroizing::new(vec![0u8; NONCE + CHUNK + MAC]);
            let mut total = 0u64;
            let mut index = 0u64;
            loop {
                progress(total)?;
                let count = read_block(&mut input, &mut buffer[..], &mut || progress(total))?;
                if count == 0 { break; }
                if count < NONCE + MAC { return Err(CryptoError::Authentication); }
                let plain_len = count - NONCE - MAC;
                let next = total.checked_add(plain_len as u64).ok_or(CryptoError::LimitExceeded)?;
                if next > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                if expected.is_some_and(|size| next > size) { return Err(CryptoError::Authentication); }
                self.chunk_mac(&header_nonce, index, &buffer[..count - MAC])?
                    .verify_slice(&buffer[count - MAC..count]).map_err(|_| CryptoError::Authentication)?;
                let (nonce, rest) = buffer.split_at_mut(NONCE);
                ctr(&file_key[..], nonce)?.apply_keystream(&mut rest[..plain_len]);
                progress(total)?;
                output.write_all(&rest[..plain_len])?;
                buffer[..].zeroize();
                total = next;
                index += 1;
                if count < NONCE + CHUNK + MAC { break; }
            }
            if expected.is_some_and(|size| size != total) { return Err(CryptoError::Authentication); }
            progress(total)?;
            Ok(CryptomatorContentOutcome { plaintext_bytes: total, trusted_length_matched: expected.is_some() })
        })();
        finish(output, result)
    }
}

/// Per-file chunk opener (random access, E4.7): MAC checked before decryption.
pub(crate) struct CtrMacChunks { file_key: Zeroizing<[u8; 32]>, mac: Zeroizing<[u8; 32]>, header_nonce: [u8; NONCE] }
pub(crate) const CTRMAC_HEADER: usize = HEADER;
pub(crate) const CTRMAC_OVERHEAD: usize = NONCE + MAC;
impl CryptomatorCtrMacKey {
    pub(crate) fn open_header(&self, header: &[u8]) -> Result<CtrMacChunks> {
        if header.len() != HEADER { return Err(CryptoError::Authentication); }
        let mut mac = self.hmac()?;
        mac.update(&header[..NONCE + 40]);
        mac.verify_slice(&header[NONCE + 40..]).map_err(|_| CryptoError::Authentication)?;
        let mut header_nonce = [0u8; NONCE];
        header_nonce.copy_from_slice(&header[..NONCE]);
        let mut payload = Zeroizing::new([0u8; 40]);
        payload.copy_from_slice(&header[NONCE..NONCE + 40]);
        ctr(&self.encryption[..], &header_nonce)?.apply_keystream(&mut payload[..]);
        let mut file_key = Zeroizing::new([0u8; 32]);
        file_key.copy_from_slice(&payload[8..]);
        Ok(CtrMacChunks { file_key, mac: self.mac.clone(), header_nonce })
    }
}
impl CtrMacChunks {
    pub(crate) fn open(&self, index: u64, sealed: &mut [u8]) -> Result<std::ops::Range<usize>> {
        if sealed.len() < NONCE + MAC || sealed.len() > NONCE + CHUNK + MAC { return Err(CryptoError::Authentication); }
        let end = sealed.len() - MAC;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.mac[..]).map_err(|_| CryptoError::InvalidInput)?;
        mac.update(&self.header_nonce);
        mac.update(&index.to_be_bytes());
        mac.update(&sealed[..end]);
        mac.verify_slice(&sealed[end..]).map_err(|_| CryptoError::Authentication)?;
        let (nonce, rest) = sealed.split_at_mut(NONCE);
        ctr(&self.file_key[..], nonce)?.apply_keystream(&mut rest[..end - NONCE]);
        Ok(NONCE..end)
    }
}
