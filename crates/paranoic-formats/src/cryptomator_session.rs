//! Revocable, single-unlock Cryptomator session. Host owns lifecycle delivery.
use std::{io::Read, sync::{Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use paranoic_crypto::{CryptoError, LifecycleEvent, Result, StreamSink};
use zeroize::Zeroizing;
use crate::{CryptomatorVault, CryptomatorDirectoryId, CryptomatorEntryName,
    CryptomatorContentLimits, CryptomatorContentOutcome, CryptomatorLengthPolicy};

/// May be shared using Arc. No API exposes the underlying vault or its keys.
/// Lock is permanent: a fresh password unlock needs a new session object.
pub struct CryptomatorSession {
    vault: Mutex<Option<CryptomatorVault>>,
    revoked: AtomicBool,
    deadline: Instant,
}

impl CryptomatorSession {
    pub fn new(vault: CryptomatorVault, lifetime: Duration) -> Result<Self> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(86400) {
            return Err(CryptoError::InvalidInput);
        }
        let deadline = Instant::now().checked_add(lifetime).ok_or(CryptoError::InvalidInput)?;
        Ok(Self { vault: Mutex::new(Some(vault)), revoked: AtomicBool::new(false), deadline })
    }

    /// Nonblocking revocation signal. Call lock off the UI thread to await
    /// operation cleanup and erase keys. Previously returned plaintext cannot
    /// be recalled. An already-started storage commit may complete.
    pub fn request_lock(&self) { self.revoked.store(true, Ordering::Release); }

    pub fn lock(&self) {
        self.request_lock();
        let mut vault = self.vault.lock().unwrap_or_else(|error| error.into_inner());
        *vault = None;
    }

    /// Foreground never revives a session. The host must call this for every
    /// live session and revoke pending unlock admissions before backgrounding.
    pub fn on_event(&self, event: LifecycleEvent) {
        if !matches!(event, LifecycleEvent::Foreground) { self.lock(); }
    }

    /// Nonblocking timer hook; true means resident keys have been dropped.
    /// Host should invoke periodically even when there are no file operations.
    pub fn expire(&self) -> bool {
        if self.check().is_ok() { return false; }
        self.request_lock();
        match self.vault.try_lock() {
            Ok(mut vault) => { *vault = None; true }
            Err(std::sync::TryLockError::WouldBlock) => false,
            Err(std::sync::TryLockError::Poisoned(error)) => {
                *error.into_inner() = None;
                true
            }
        }
    }

    pub fn id(&self) -> Result<uuid::Uuid> { self.with_vault(|vault| Ok(vault.id())) }
    pub fn format(&self) -> Result<u32> { self.with_vault(|vault| Ok(vault.format())) }
    pub fn cipher_combo(&self) -> Result<crate::CryptomatorCipherCombo> { self.with_vault(|vault| Ok(vault.cipher_combo())) }
    pub fn directory_path(&self, id: &CryptomatorDirectoryId) -> Result<String> {
        self.with_vault(|vault| vault.directory_path(id))
    }
    pub fn encrypt_name(&self, parent: &CryptomatorDirectoryId, name: &str) -> Result<CryptomatorEntryName> {
        self.with_vault(|vault| vault.encrypt_name(parent, name))
    }
    pub fn decrypt_name(&self, parent: &CryptomatorDirectoryId, name: &str, mapping: Option<&str>)
        -> Result<Zeroizing<String>>
    {
        self.with_vault(|vault| vault.decrypt_name(parent, name, mapping))
    }
    pub(crate) fn open_chunks(&self, header: &[u8]) -> Result<crate::cryptomator_random::Chunks> {
        self.with_vault(|vault| vault.open_chunks(header))
    }
    pub fn directory_backup(&self, id: &CryptomatorDirectoryId) -> Result<Vec<u8>> {
        self.with_vault(|vault| vault.directory_backup(id, || self.check()))
    }
    pub fn prepare_password_change(&self, password: Zeroizing<String>) -> Result<Vec<u8>> {
        self.with_vault(|vault| vault.prepare_password_change(password, || self.check()))
    }

    pub fn encrypt_content<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, input: R, output: Box<dyn StreamSink>, limits: CryptomatorContentLimits,
        mut progress: F,
    ) -> Result<u64> {
        self.with_stream(output, |vault, output| vault.encrypt_content(input, output, limits, |bytes| {
            self.check()?;
            progress(bytes)?;
            self.check()
        }))
    }

    pub fn decrypt_content<R: Read, F: FnMut(u64) -> Result<()>>(
        &self, input: R, output: Box<dyn StreamSink>, limits: CryptomatorContentLimits,
        length: CryptomatorLengthPolicy, mut progress: F,
    ) -> Result<CryptomatorContentOutcome> {
        self.with_stream(output, |vault, output| vault.decrypt_content(input, output, limits, length, |bytes| {
            self.check()?;
            progress(bytes)?;
            self.check()
        }))
    }

    fn check(&self) -> Result<()> {
        if self.revoked.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err(CryptoError::Locked)
        } else { Ok(()) }
    }

    fn with_vault<T>(&self, operation: impl FnOnce(&CryptomatorVault) -> Result<T>) -> Result<T> {
        let mut guard = match self.vault.lock() {
            Ok(guard) => guard,
            Err(error) => {
                self.request_lock();
                *error.into_inner() = None;
                return Err(CryptoError::Locked);
            }
        };
        if self.check().is_err() { *guard = None; return Err(CryptoError::Locked); }
        let result = operation(guard.as_ref().ok_or(CryptoError::Locked)?);
        if self.check().is_err() {
            *guard = None;
            // Decrypted names own Zeroizing buffers and are erased here.
            return Err(CryptoError::Locked);
        }
        result
    }

    fn with_stream<T>(&self, output: Box<dyn StreamSink>,
        operation: impl FnOnce(&CryptomatorVault, Box<dyn StreamSink>) -> Result<T>,
    ) -> Result<T> {
        // Reject admission before codec allocation or waiting for a session.
        // Cleanup failure takes precedence: the host must see retained output.
        if let Err(error) = self.check() {
            output.abort().map_err(|_| CryptoError::CleanupFailed)?;
            return Err(error);
        }
        let _permit = match crate::cryptomator_budget::StreamPermit::acquire() {
            Ok(permit) => permit,
            Err(error) => {
                output.abort().map_err(|_| CryptoError::CleanupFailed)?;
                return Err(error);
            }
        };
        let mut guard = match self.vault.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                output.abort().map_err(|_| CryptoError::CleanupFailed)?;
                return Err(if self.check().is_err() { CryptoError::Locked } else { CryptoError::Busy });
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                self.request_lock();
                *error.into_inner() = None;
                output.abort().map_err(|_| CryptoError::CleanupFailed)?;
                return Err(CryptoError::Locked);
            }
        };
        if self.check().is_err() { *guard = None; }
        let Some(vault) = guard.as_ref() else {
            output.abort().map_err(|_| CryptoError::CleanupFailed)?;
            return Err(CryptoError::Locked);
        };
        let result = operation(vault, output);
        if self.check().is_err() { *guard = None; }
        // Publication wins if revocation races a storage commit: never turn
        // success/PublicationUncertain into a claim of clean cancellation.
        result
    }

}

impl Drop for CryptomatorSession {
    fn drop(&mut self) {
        self.revoked.store(true, Ordering::Release);
        let vault = self.vault.get_mut().unwrap_or_else(|error| error.into_inner());
        *vault = None;
    }
}
