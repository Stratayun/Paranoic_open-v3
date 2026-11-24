use std::{collections::HashMap, io::Read, sync::{Arc, Condvar, Mutex}, thread::{self, JoinHandle}, time::Duration};

use zeroize::Zeroizing;

use crate::{CryptoError, KeyRecord, KeySession, ObjectId, PasswordKeyring, Result, StreamLimits, StreamSink};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEvent { Background, ScreenLocked, Suspend, Foreground, Shutdown }

/// A handle cannot unlock or replace its resident key. Old handles stay revoked
/// even if the manager opens a new session for the same persistent key record.
#[derive(Clone)]
pub struct SessionHandle {
    id: ObjectId,
    session: Arc<KeySession>,
}

impl SessionHandle {
    pub fn id(&self) -> ObjectId { self.id }
    pub fn seal(&self, plaintext: &[u8], context: &[u8]) -> Result<Vec<u8>> { self.session.seal(plaintext, context) }
    pub fn open(&self, ciphertext: &[u8], context: &[u8]) -> Result<Zeroizing<Vec<u8>>> { self.session.open(ciphertext, context) }
    pub fn encrypt_stream<R: Read>(&self, input: R, output: Box<dyn StreamSink>, context: &[u8], limits: StreamLimits,
        checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        self.session.encrypt_stream(input, output, context, limits, checkpoint)
    }
    pub fn decrypt_stream<R: Read>(&self, input: R, output: Box<dyn StreamSink>, context: &[u8], limits: StreamLimits,
        checkpoint: impl FnMut(u64) -> Result<()>) -> Result<u64> {
        self.session.decrypt_stream(input, output, context, limits, checkpoint)
    }
}

struct State {
    epoch: u64,
    accepting: bool,
    shutdown: bool,
    pending: usize,
    credential_change: bool,
    sessions: HashMap<ObjectId, Arc<KeySession>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    capacity: usize,
}

/// Owns a timer that clears idle expired keys without requiring UI polling.
/// Host forwards real lifecycle events; this module does not subscribe to OS APIs.
pub struct SessionManager {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl SessionManager {
    pub fn new(capacity: usize) -> Result<Self> {
        if !(1..=128).contains(&capacity) { return Err(CryptoError::InvalidInput); }
        let shared = Arc::new(Shared {
            state: Mutex::new(State { epoch: 0, accepting: true, shutdown: false, pending: 0, credential_change: false, sessions: HashMap::new() }),
            wake: Condvar::new(), capacity,
        });
        let background = Arc::clone(&shared);
        let worker = thread::Builder::new().name("paranoic-key-expiry".into()).spawn(move || expire_loop(background))
            .map_err(|_| CryptoError::Io)?;
        Ok(Self { shared, worker: Some(worker) })
    }

    /// Perform password derivation off the UI thread. A lifecycle event during
    /// derivation invalidates admission, even after a subsequent Foreground.
    pub fn unlock(&self, keyring: &PasswordKeyring, record: KeyRecord, password: &[u8], lifetime: Duration) -> Result<SessionHandle> {
        let admission = Admission::reserve(&self.shared)?;
        let session = Arc::new(keyring.unlock(record, password, lifetime)?);
        let id = match ObjectId::generate() {
            Ok(id) => id,
            Err(error) => { session.lock(); return Err(error); }
        };
        let mut state = self.shared.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown || !state.accepting || state.credential_change || state.epoch != admission.epoch {
            drop(state);
            session.lock();
            return Err(CryptoError::Locked);
        }
        if state.sessions.contains_key(&id) {
            drop(state);
            session.lock();
            return Err(CryptoError::AlreadyExists);
        }
        state.sessions.insert(id, Arc::clone(&session));
        drop(state);
        drop(admission);
        Ok(SessionHandle { id, session })
    }

    /// Revokes all handles and erases resident keys without waiting for
    /// streams (they abort at their next checkpoint; see KeySession::wait_idle).
    /// Foreground only admits fresh password unlocks and never reopens a
    /// previous handle.
    pub fn on_event(&self, event: LifecycleEvent) -> Result<()> {
        let mut state = self.shared.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown { return if event == LifecycleEvent::Shutdown { Ok(()) } else { Err(CryptoError::Locked) }; }
        if event == LifecycleEvent::Foreground {
            if state.credential_change { return Err(CryptoError::Locked); }
            state.accepting = true;
            return Ok(());
        }
        state.accepting = false;
        state.shutdown = event == LifecycleEvent::Shutdown;
        // Overflow must fail closed instead of reusing an admission epoch.
        match state.epoch.checked_add(1) {
            Some(epoch) => state.epoch = epoch,
            None => state.shutdown = true,
        }
        for session in state.sessions.values() { session.request_lock(); }
        let revoked: Vec<_> = state.sessions.drain().map(|(_, session)| session).collect();
        self.shared.wake.notify_all();
        drop(state);
        for session in revoked { session.lock(); }
        Ok(())
    }

    pub fn lock_session(&self, id: ObjectId) -> Result<()> {
        let session = {
            let mut state = self.shared.state.lock().map_err(|_| CryptoError::Locked)?;
            let session = state.sessions.remove(&id).ok_or(CryptoError::NotFound)?;
            session.request_lock();
            session
        };
        session.lock();
        Ok(())
    }

    pub(crate) fn while_locked<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        {
            let mut state = self.shared.state.lock().map_err(|_| CryptoError::Locked)?;
            if state.shutdown { return Err(CryptoError::Locked); }
            if state.credential_change { return Err(CryptoError::Busy); }
            state.credential_change = true;
        }
        let _barrier = CredentialBarrier(Arc::clone(&self.shared));
        self.on_event(LifecycleEvent::Background)?;
        operation()
    }
}

struct CredentialBarrier(Arc<Shared>);

impl Drop for CredentialBarrier {
    fn drop(&mut self) {
        let mut state = match self.0.state.lock() { Ok(state) => state, Err(error) => error.into_inner() };
        state.credential_change = false;
        // Explicit foreground is required after success, error or unwinding.
        state.accepting = false;
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // Recover poison solely for revocation/cleanup; never reopen access.
        let mut state = match self.shared.state.lock() {
            Ok(state) => state, Err(error) => error.into_inner(),
        };
        state.shutdown = true;
        state.accepting = false;
        for session in state.sessions.values() { session.request_lock(); }
        let revoked: Vec<_> = state.sessions.drain().map(|(_, session)| session).collect();
        self.shared.wake.notify_all();
        drop(state);
        for session in revoked { session.lock(); }
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

struct Admission { shared: Arc<Shared>, epoch: u64 }

impl Admission {
    fn reserve(shared: &Arc<Shared>) -> Result<Self> {
        let mut state = shared.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown || !state.accepting || state.credential_change { return Err(CryptoError::Locked); }
        if state.sessions.len() + state.pending >= shared.capacity { return Err(CryptoError::Busy); }
        state.pending += 1;
        Ok(Self { shared: Arc::clone(shared), epoch: state.epoch })
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut state = match self.shared.state.lock() { Ok(state) => state, Err(error) => error.into_inner() };
        state.pending = state.pending.saturating_sub(1);
    }
}

fn expire_loop(shared: Arc<Shared>) {
    loop {
        let mut state = match shared.state.lock() { Ok(state) => state, Err(_) => return };
        if state.shutdown { return; }
        // try_expire never waits on a busy crypto session. Streaming operations
        // perform their own deadline checks while holding the key mutex.
        state.sessions.retain(|_, session| !session.try_expire());
        let result = shared.wake.wait_timeout(state, Duration::from_secs(1));
        match result {
            Ok((state, _)) if state.shutdown => return,
            Ok(_) => {},
            Err(_) => return,
        }
    }
}
