use std::io::{Read, Write};

use chacha20poly1305::aead::{stream::{DecryptorBE32, EncryptorBE32}, Payload};
use zeroize::Zeroizing;

use crate::{associated_data, random, validate_context, CryptoError, Result, SecretKey};

const MAGIC: &[u8; 8] = b"PRNSTR01";
const CHUNK: usize = 1024 * 1024;
const HEADER: usize = 31;

/// Private tentative output. Writes MUST NOT expose plaintext to ordinary file
/// browsing, caches or consumers. commit publishes only on complete success.
/// Drop must also abandon tentative output, including on unwinding.
pub trait StreamSink: Write {
    fn commit(self: Box<Self>) -> Result<()>;
    fn abort(self: Box<Self>) -> Result<()>;
}

#[derive(Clone, Copy, Debug)]
pub struct StreamLimits {
    pub max_plaintext_bytes: u64,
}

impl Default for StreamLimits {
    fn default() -> Self { Self { max_plaintext_bytes: 1 << 40 } }
}

impl StreamLimits {
    fn validate(self) -> Result<()> {
        // Stay well below STREAM's 32-bit counter exhaustion, even for the
        // mandatory final record after an exact multiple of CHUNK bytes.
        if self.max_plaintext_bytes == 0 || self.max_plaintext_bytes > (1_u64 << 40) {
            return Err(CryptoError::InvalidInput);
        }
        Ok(())
    }
}

impl SecretKey {
    pub fn encrypt_stream<R: Read>(
        &self, mut input: R, mut output: Box<dyn StreamSink>, context: &[u8],
        limits: StreamLimits, mut checkpoint: impl FnMut(u64) -> Result<()>,
    ) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            validate_context(context)?;
            checkpoint(0)?;
            let mut nonce = [0_u8; 19];
            random(&mut nonce)?;
            let mut header = [0_u8; HEADER];
            header[..8].copy_from_slice(MAGIC);
            header[8..27].copy_from_slice(&nonce);
            header[27..].copy_from_slice(&(CHUNK as u32).to_le_bytes());
            let mut aad = associated_data(MAGIC, context);
            aad.extend_from_slice(&header);
            let mut encryptor = EncryptorBE32::from_aead(self.cipher()?, (&nonce).into());
            output.write_all(&header).map_err(|_| CryptoError::Io)?;
            let mut plaintext = Zeroizing::new(vec![0_u8; CHUNK]);
            let mut total = 0_u64;
            loop {
                let mut filled = 0;
                while filled < CHUNK {
                    checkpoint(total)?;
                    let count = read(&mut input, &mut plaintext[filled..])?;
                    if count == 0 { break; }
                    total = total.checked_add(count as u64).ok_or(CryptoError::LimitExceeded)?;
                    if total > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                    filled += count;
                }
                checkpoint(total)?;
                let payload = Payload { msg: &plaintext[..filled], aad: &aad };
                if filled < CHUNK {
                    let ciphertext = encryptor.encrypt_last(payload).map_err(|_| CryptoError::Authentication)?;
                    write_record(output.as_mut(), true, &ciphertext)?;
                    break;
                }
                let ciphertext = encryptor.encrypt_next(payload).map_err(|_| CryptoError::Authentication)?;
                write_record(output.as_mut(), false, &ciphertext)?;
            }
            checkpoint(total)?;
            output.flush().map_err(|_| CryptoError::Io)?;
            checkpoint(total)?;
            Ok(total)
        })();
        finish(output, result)
    }

    pub fn decrypt_stream<R: Read>(
        &self, mut input: R, mut output: Box<dyn StreamSink>, context: &[u8],
        limits: StreamLimits, mut checkpoint: impl FnMut(u64) -> Result<()>,
    ) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            validate_context(context)?;
            checkpoint(0)?;
            let mut header = [0_u8; HEADER];
            exact(&mut input, &mut header)?;
            if &header[..8] != MAGIC || header[27..] != (CHUNK as u32).to_le_bytes() {
                return Err(CryptoError::InvalidInput);
            }
            let nonce: &[u8; 19] = header[8..27].try_into().map_err(|_| CryptoError::InvalidInput)?;
            let mut aad = associated_data(MAGIC, context);
            aad.extend_from_slice(&header);
            let mut decryptor = DecryptorBE32::from_aead(self.cipher()?, nonce.into());
            let mut total = 0_u64;
            loop {
                checkpoint(total)?;
                let mut frame = [0_u8; 5];
                exact(&mut input, &mut frame)?;
                let last = match frame[0] { 0 => false, 1 => true, _ => return Err(CryptoError::InvalidInput) };
                let length = u32::from_le_bytes(frame[1..].try_into().map_err(|_| CryptoError::InvalidInput)?) as usize;
                if (last && !(16..CHUNK + 16).contains(&length)) || (!last && length != CHUNK + 16) {
                    return Err(CryptoError::InvalidInput);
                }
                total = total.checked_add((length - 16) as u64).ok_or(CryptoError::LimitExceeded)?;
                if total > limits.max_plaintext_bytes { return Err(CryptoError::LimitExceeded); }
                let mut ciphertext = vec![0_u8; length];
                exact(&mut input, &mut ciphertext)?;
                let payload = Payload { msg: &ciphertext, aad: &aad };
                if last {
                    let plaintext = Zeroizing::new(decryptor.decrypt_last(payload).map_err(|_| CryptoError::Authentication)?);
                    // Reject appended garbage and concatenated streams. No
                    // publication is allowed before both final tag and EOF.
                    let mut trailing = [0_u8; 1];
                    if read(&mut input, &mut trailing)? != 0 { return Err(CryptoError::Authentication); }
                    checkpoint(total)?;
                    output.write_all(&plaintext).map_err(|_| CryptoError::Io)?;
                    break;
                }
                let plaintext = Zeroizing::new(decryptor.decrypt_next(payload).map_err(|_| CryptoError::Authentication)?);
                checkpoint(total)?;
                output.write_all(&plaintext).map_err(|_| CryptoError::Io)?;
            }
            checkpoint(total)?;
            output.flush().map_err(|_| CryptoError::Io)?;
            checkpoint(total)?;
            Ok(total)
        })();
        finish(output, result)
    }
}

fn read(input: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
    loop {
        match input.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result.map_err(|_| CryptoError::Io),
        }
    }
}

fn exact(input: &mut impl Read, buffer: &mut [u8]) -> Result<()> {
    input.read_exact(buffer).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof { CryptoError::Authentication } else { CryptoError::Io }
    })
}

fn write_record(output: &mut dyn StreamSink, last: bool, ciphertext: &[u8]) -> Result<()> {
    output.write_all(&[u8::from(last)]).map_err(|_| CryptoError::Io)?;
    output.write_all(&(ciphertext.len() as u32).to_le_bytes()).map_err(|_| CryptoError::Io)?;
    output.write_all(ciphertext).map_err(|_| CryptoError::Io)
}

fn finish(output: Box<dyn StreamSink>, result: Result<u64>) -> Result<u64> {
    match result {
        Ok(bytes) => { output.commit()?; Ok(bytes) }
        Err(error) => {
            output.abort().map_err(|_| CryptoError::CleanupFailed)?;
            Err(error)
        }
    }
}
