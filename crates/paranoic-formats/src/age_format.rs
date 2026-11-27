use std::{cell::Cell, io::{self, Read, Write}, rc::Rc};

use age::secrecy::{ExposeSecret, SecretString};
use paranoic_crypto::{CryptoError, Result, StreamSink};
use zeroize::Zeroizing;

/// Secret material is deliberately neither Debug nor serializable. Export is
/// explicit and returns a secret container; caller must encrypt it for storage.
pub struct AgeIdentity(age::x25519::Identity);

impl AgeIdentity {
    pub fn generate() -> Self { Self(age::x25519::Identity::generate()) }
    pub fn import(secret: SecretString) -> Result<Self> {
        if secret.expose_secret().len() > 256 { return Err(CryptoError::InvalidInput); }
        secret.expose_secret().parse().map(Self).map_err(|_| CryptoError::InvalidInput)
    }
    pub fn recipient(&self) -> String { self.0.to_public().to_string() }
    pub fn export(&self) -> SecretString { self.0.to_string() }
}

#[derive(Clone, Copy, Debug)]
pub struct AgeLimits {
    pub max_plaintext_bytes: u64,
    pub max_header_bytes: u64,
    pub max_scrypt_work_factor: u8,
}

impl Default for AgeLimits {
    fn default() -> Self {
        Self { max_plaintext_bytes: 1 << 40, max_header_bytes: 64 * 1024, max_scrypt_work_factor: 18 }
    }
}

impl AgeLimits {
    fn validate(self) -> Result<()> {
        if self.max_plaintext_bytes == 0 || self.max_plaintext_bytes > (1 << 40)
            || !(1024..=1024 * 1024).contains(&self.max_header_bytes)
            || !(14..=18).contains(&self.max_scrypt_work_factor)
        { return Err(CryptoError::InvalidInput); }
        Ok(())
    }

    fn ciphertext_budget(self) -> u64 {
        self.max_plaintext_bytes + (self.max_plaintext_bytes / 65_536 + 1) * 16 + self.max_header_bytes + 16
    }
}

pub struct AgeFormat;

impl AgeFormat {
    pub fn encrypt_recipients<R: Read>(input: R, mut output: Box<dyn StreamSink>, recipients: &[String], limits: AgeLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            checkpoint(0)?;
            if recipients.is_empty() || recipients.len() > 64 { return Err(CryptoError::InvalidInput); }
            let parsed: Vec<age::x25519::Recipient> = recipients.iter().map(|value| {
                if value.len() > 256 { return Err(CryptoError::InvalidInput); }
                value.parse().map_err(|_| CryptoError::InvalidInput)
            }).collect::<Result<_>>()?;
            let encryptor = age::Encryptor::with_recipients(parsed.iter().map(|recipient| recipient as &dyn age::Recipient))
                .map_err(|_| CryptoError::InvalidInput)?;
            encrypt(input, output.as_mut(), encryptor, limits, &mut checkpoint)
        })();
        finish(output, result)
    }

    pub fn encrypt_password<R: Read>(input: R, mut output: Box<dyn StreamSink>, password: SecretString, limits: AgeLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            password_valid(&password)?;
            checkpoint(0)?;
            let mut recipient = age::scrypt::Recipient::new(password);
            recipient.set_work_factor(limits.max_scrypt_work_factor);
            // age 0.11 runs scrypt here; the shared KDF budget (A28) covers it.
            let permit = paranoic_crypto::kdf_gate::admit(scrypt_kib(limits.max_scrypt_work_factor), &mut || checkpoint(0))?;
            let encryptor = age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .map_err(|_| CryptoError::InvalidInput)?;
            drop(permit);
            checkpoint(0)?;
            encrypt(input, output.as_mut(), encryptor, limits, &mut checkpoint)
        })();
        finish(output, result)
    }

    pub fn decrypt_identity<R: Read>(input: R, mut output: Box<dyn StreamSink>, identity: &AgeIdentity, limits: AgeLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            checkpoint(0)?;
            let (reader, budget) = BudgetReader::new(input, limits.max_header_bytes);
            let decryptor = age::Decryptor::new(reader).map_err(|_| CryptoError::Authentication)?;
            if decryptor.is_scrypt() { return Err(CryptoError::Unsupported); }
            budget.set(limits.ciphertext_budget());
            checkpoint(0)?;
            let reader = decryptor.decrypt(std::iter::once(&identity.0 as &dyn age::Identity))
                .map_err(|_| CryptoError::Authentication)?;
            pump(reader, output.as_mut(), limits.max_plaintext_bytes, true, &mut checkpoint)
        })();
        finish(output, result)
    }

    pub fn decrypt_password<R: Read>(input: R, mut output: Box<dyn StreamSink>, password: SecretString, limits: AgeLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        let result = (|| {
            limits.validate()?;
            password_valid(&password)?;
            checkpoint(0)?;
            let (reader, budget) = BudgetReader::new(input, limits.max_header_bytes);
            let decryptor = age::Decryptor::new(reader).map_err(|_| CryptoError::Authentication)?;
            if !decryptor.is_scrypt() { return Err(CryptoError::Unsupported); }
            budget.set(limits.ciphertext_budget());
            let mut identity = age::scrypt::Identity::new(password);
            identity.set_max_work_factor(limits.max_scrypt_work_factor);
            checkpoint(0)?;
            let permit = paranoic_crypto::kdf_gate::admit(scrypt_kib(limits.max_scrypt_work_factor), &mut || checkpoint(0))?;
            let reader = decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity))
                .map_err(|_| CryptoError::Authentication)?;
            drop(permit);
            checkpoint(0)?;
            pump(reader, output.as_mut(), limits.max_plaintext_bytes, true, &mut checkpoint)
        })();
        finish(output, result)
    }
}

fn encrypt<R: Read>(input: R, output: &mut dyn StreamSink, encryptor: age::Encryptor, limits: AgeLimits,
    checkpoint: &mut impl FnMut(u64) -> Result<()>) -> Result<u64> {
    let mut writer = encryptor.wrap_output(&mut *output).map_err(|_| CryptoError::Io)?;
    let bytes = pump(input, &mut writer, limits.max_plaintext_bytes, false, checkpoint)?;
    checkpoint(bytes)?;
    writer.finish().map_err(|_| CryptoError::Io)?;
    output.flush().map_err(|_| CryptoError::Io)?;
    checkpoint(bytes)?;
    Ok(bytes)
}

fn pump<R: Read, W: Write + ?Sized>(mut input: R, output: &mut W, maximum: u64, decrypting: bool,
    checkpoint: &mut impl FnMut(u64) -> Result<()>) -> Result<u64> {
    let mut buffer = Zeroizing::new(vec![0_u8; 64 * 1024]);
    let mut total = 0_u64;
    loop {
        checkpoint(total)?;
        let count = match input.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(|_| if decrypting { CryptoError::Authentication } else { CryptoError::Io })?,
        };
        if count == 0 { break; }
        total = total.checked_add(count as u64).ok_or(CryptoError::LimitExceeded)?;
        if total > maximum { return Err(CryptoError::LimitExceeded); }
        checkpoint(total)?;
        output.write_all(&buffer[..count]).map_err(|_| CryptoError::Io)?;
    }
    output.flush().map_err(|_| CryptoError::Io)?;
    checkpoint(total)?;
    Ok(total)
}

/// scrypt memory with r = 8: 128 · 8 · 2^log_n bytes = 2^log_n KiB (upper bound
/// for decryption, whose header may ask for less).
fn scrypt_kib(log_n: u8) -> u64 { 1u64 << log_n }

fn finish(output: Box<dyn StreamSink>, result: Result<u64>) -> Result<u64> {
    match result {
        Ok(bytes) => { output.commit()?; Ok(bytes) },
        Err(error) => { output.abort().map_err(|_| CryptoError::CleanupFailed)?; Err(error) },
    }
}

fn password_valid(password: &SecretString) -> Result<()> {
    if password.expose_secret().is_empty() || password.expose_secret().len() > 4096 {
        Err(CryptoError::InvalidInput)
    } else { Ok(()) }
}

/// Limits header allocation BEFORE age parses recipient stanzas. The parser
/// owns this reader; a shared limit allows switching to a bounded payload budget.
struct BudgetReader<R> { input: R, consumed: u64, limit: Rc<Cell<u64>> }

impl<R: Read> BudgetReader<R> {
    fn new(input: R, maximum: u64) -> (Self, Rc<Cell<u64>>) {
        let limit = Rc::new(Cell::new(maximum));
        (Self { input, consumed: 0, limit: Rc::clone(&limit) }, limit)
    }
}

impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() { return Ok(0); }
        let remaining = self.limit.get().saturating_sub(self.consumed);
        if remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.input.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(io::ErrorKind::InvalidData, "age input budget exceeded")),
            };
        }
        let requested = remaining.min(buffer.len() as u64) as usize;
        let count = self.input.read(&mut buffer[..requested])?;
        self.consumed += count as u64;
        Ok(count)
    }
}
