use std::{sync::Mutex, time::{Duration, Instant}};

use zeroize::Zeroizing;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{CryptoError, Result, SecretKey, StreamLimits, StreamSink};

struct Unlocked {
    key: SecretKey,
    deadline: Instant,
}

/// Single-key session. Bounded seal/open hold the key mutex for their short
/// duration. Streams copy the key under the mutex and run WITHOUT it, so
/// lock() never waits for storage I/O: it revokes access and erases the
/// resident key immediately. A running stream keeps its own copy until its
/// next checkpoint (one chunk), then aborts and drops it; a stream blocked in
/// provider I/O keeps it until that call returns. `in_flight()`/`wait_idle()`
/// tell "access revoked" apart from "all key copies erased".
#[derive(Default)]
pub struct KeySession {
    key: Mutex<Option<Unlocked>>,
    lock_requested: AtomicBool,
    streams: AtomicUsize,
}

impl KeySession {
    pub fn new() -> Self { Self::default() }

    /// Ownership moves into the session. Host must authorize this transition.
    /// Password verification can occur first using SecretKey::unwrap.
    pub fn unlock(&self, key: SecretKey, lifetime: Duration) -> Result<()> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(24 * 60 * 60) {
            return Err(CryptoError::InvalidInput);
        }
        let deadline = Instant::now().checked_add(lifetime).ok_or(CryptoError::InvalidInput)?;
        let mut guard = self.key.lock().map_err(|_| CryptoError::Locked)?;
        *guard = Some(Unlocked { key, deadline });
        self.lock_requested.store(false, Ordering::Release);
        Ok(())
    }

    pub fn lock(&self) {
        self.request_lock();
        // Even after a panic, recover the guard solely to erase its key. Normal
        // operations continue to reject the poisoned mutex, failing closed.
        let mut guard = match self.key.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = None;
    }

    /// Nonblocking access revocation. Call lock() off the UI thread afterwards
    /// to wait for in-flight cleanup and erase the key. Already returned
    /// plaintext is owned by the caller and cannot be revoked by this method.
    pub fn request_lock(&self) {
        self.lock_requested.store(true, Ordering::Release);
    }

    /// Streams still holding a per-operation key copy.
    pub fn in_flight(&self) -> usize { self.streams.load(Ordering::Acquire) }

    /// Wait until no stream holds a key copy. false = still busy at timeout
    /// (e.g. provider I/O hung); access is revoked regardless.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        while self.in_flight() != 0 {
            if Instant::now() >= until { return false; }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    pub fn expire(&self) -> Result<bool> {
        let mut guard = self.key.lock().map_err(|_| CryptoError::Locked)?;
        if guard.as_ref().is_some_and(|value| Instant::now() >= value.deadline) {
            *guard = None;
        }
        Ok(guard.is_none())
    }

    pub(crate) fn try_expire(&self) -> bool {
        let mut guard = match self.key.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return false,
            Err(std::sync::TryLockError::Poisoned(error)) => {
                self.request_lock();
                let mut guard = error.into_inner();
                *guard = None;
                return true;
            }
        };
        if self.lock_requested.load(Ordering::Acquire)
            || guard.as_ref().is_some_and(|value| Instant::now() >= value.deadline)
        {
            *guard = None;
        }
        guard.is_none()
    }

    pub fn seal(&self, plaintext: &[u8], context: &[u8]) -> Result<Vec<u8>> {
        self.with_key(|key| key.seal(plaintext, context))
    }

    pub fn open(&self, envelope: &[u8], context: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        self.with_key(|key| key.open(envelope, context))
    }

    /// Returns only the new encrypted wrapper. Persist it atomically before
    /// discarding the old wrapper; this API performs no filesystem writes.
    pub fn wrap(&self, password: &[u8], context: &[u8]) -> Result<Vec<u8>> {
        self.with_key(|key| key.wrap(password, context))
    }

    /// The caller's callback can return Cancelled. Expiry is checked between
    /// chunks and before publication; blocked OS I/O is not forcibly interrupted.
    pub fn encrypt_stream<R: std::io::Read>(
        &self, input: R, output: Box<dyn StreamSink>, context: &[u8], limits: StreamLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>,
    ) -> Result<u64> {
        self.with_stream(output, |key, deadline, output| {
            key.encrypt_stream(input, output, context, limits, |bytes| {
                if self.lock_requested.load(Ordering::Acquire) || Instant::now() >= deadline { return Err(CryptoError::Locked); }
                checkpoint(bytes)
            })
        })
    }

    pub fn decrypt_stream<R: std::io::Read>(
        &self, input: R, output: Box<dyn StreamSink>, context: &[u8], limits: StreamLimits,
        mut checkpoint: impl FnMut(u64) -> Result<()>,
    ) -> Result<u64> {
        self.with_stream(output, |key, deadline, output| {
            key.decrypt_stream(input, output, context, limits, |bytes| {
                if self.lock_requested.load(Ordering::Acquire) || Instant::now() >= deadline { return Err(CryptoError::Locked); }
                checkpoint(bytes)
            })
        })
    }

    fn with_stream<T>(
        &self, output: Box<dyn StreamSink>,
        operation: impl FnOnce(&SecretKey, Instant, Box<dyn StreamSink>) -> Result<T>,
    ) -> Result<T> {
        // Short critical section: check state and take a per-operation copy.
        let taken = match self.key.lock() {
            Ok(mut guard) => {
                if self.lock_requested.load(Ordering::Acquire) || guard.as_ref().is_some_and(|value| Instant::now() >= value.deadline) { *guard = None; }
                guard.as_ref().map(|value| (value.key.duplicate(), value.deadline))
            }
            Err(_) => None,
        };
        let Some((key, deadline)) = taken else {
            output.abort().map_err(|_| CryptoError::CleanupFailed)?;
            return Err(CryptoError::Locked);
        };
        self.streams.fetch_add(1, Ordering::AcqRel);
        struct Running<'a>(&'a AtomicUsize);
        impl Drop for Running<'_> { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); } }
        let _running = Running(&self.streams);
        // A successful commit wins even if the deadline passed inside the
        // storage commit call. Never report a clean rollback after publication.
        let result = operation(&key, deadline, output);
        drop(key);
        if Instant::now() >= deadline {
            if let Ok(mut guard) = self.key.lock() { *guard = None; }
        }
        result
    }

    fn with_key<T>(&self, operation: impl FnOnce(&SecretKey) -> Result<T>) -> Result<T> {
        let mut guard = self.key.lock().map_err(|_| CryptoError::Locked)?;
        if self.lock_requested.load(Ordering::Acquire) || guard.as_ref().is_some_and(|value| Instant::now() >= value.deadline) {
            *guard = None;
        }
        let value = guard.as_ref().ok_or(CryptoError::Locked)?;
        let result = operation(&value.key);
        // A bounded operation can cross the deadline. Suppress its output and
        // erase the resident key; decrypted output is Zeroizing and drops here.
        if self.lock_requested.load(Ordering::Acquire) || Instant::now() >= value.deadline {
            *guard = None;
            return Err(CryptoError::Locked);
        }
        result
    }
}
