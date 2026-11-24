//! Host-owned admission and lifecycle for Cryptomator readers.
use std::{sync::{Arc, Mutex, Weak}, time::Duration};
use paranoic_core::{ResourcePath, Storage};
use paranoic_crypto::{CryptoError, LifecycleEvent, Result};
use zeroize::Zeroizing;
use crate::{CryptomatorReader, CryptomatorSession};

struct State {
    epoch: u64,
    accepting: bool,
    shutdown: bool,
    pending: usize,
    sessions: Vec<Weak<CryptomatorSession>>,
}

/// Own one manager per host. Pending opens count against both admission limits.
/// Call expire periodically and deliver OS events; no platform hooks are implied.
pub struct CryptomatorManager {
    state: Mutex<State>,
    capacity: usize,
    parallel_opens: usize,
}

impl CryptomatorManager {
    pub fn new(capacity: usize, parallel_opens: usize) -> Result<Self> {
        if !(1..=32).contains(&capacity) || !(1..=2).contains(&parallel_opens) || parallel_opens > capacity {
            return Err(CryptoError::InvalidInput);
        }
        Ok(Self { state: Mutex::new(State { epoch: 0, accepting: true, shutdown: false,
            pending: 0, sessions: Vec::new() }), capacity, parallel_opens })
    }

    pub fn open_reader<F: FnMut() -> Result<()>>(
        &self, storage: Arc<dyn Storage>, root: ResourcePath, password: Zeroizing<String>,
        lifetime: Duration, mut checkpoint: F,
    ) -> Result<CryptomatorReader> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(86400) { return Err(CryptoError::InvalidInput); }
        let permit = self.admit()?;
        let reader = CryptomatorReader::open(storage, root, password, lifetime, || {
            permit.check()?;
            checkpoint()?;
            permit.check()
        })?;
        // Same lock as lifecycle revocation: a late successful KDF cannot
        // register a usable session after Background or a new foreground epoch.
        permit.register(&reader.session)?;
        Ok(reader)
    }

    /// Signals all readers before waiting for any one reader's I/O cleanup.
    /// Foreground enables only new opens. Shutdown is irreversible.
    pub fn on_event(&self, event: LifecycleEvent) -> Result<()> {
        let sessions = {
            let mut state = self.state.lock().map_err(|_| CryptoError::Locked)?;
            if state.shutdown {
                return if event == LifecycleEvent::Shutdown { Ok(()) } else { Err(CryptoError::Locked) };
            }
            if event == LifecycleEvent::Foreground { state.accepting = true; return Ok(()); }
            state.accepting = false;
            state.shutdown = event == LifecycleEvent::Shutdown;
            if let Some(next) = state.epoch.checked_add(1) { state.epoch = next; }
            else { state.shutdown = true; }
            let sessions: Vec<_> = state.sessions.iter().filter_map(Weak::upgrade).collect();
            for session in &sessions { session.request_lock(); }
            sessions
        };
        for session in sessions { session.lock(); }
        self.expire()
    }

    /// Nonblocking with respect to vault I/O. Clears expired/revoked idle keys
    /// and removes dead registrations; host timer should call this regularly.
    pub fn expire(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| CryptoError::Locked)?;
        prune(&mut state);
        Ok(())
    }

    fn admit(&self) -> Result<OpenPermit<'_>> {
        let mut state = self.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown || !state.accepting { return Err(CryptoError::Locked); }
        prune(&mut state);
        if state.pending >= self.parallel_opens { return Err(CryptoError::Busy); }
        if state.sessions.len() + state.pending >= self.capacity { return Err(CryptoError::LimitExceeded); }
        state.pending += 1;
        Ok(OpenPermit { manager: self, epoch: state.epoch })
    }
}

fn prune(state: &mut State) {
    state.sessions.retain(|weak| weak.upgrade().is_some_and(|session| !session.expire()));
}

struct OpenPermit<'a> { manager: &'a CryptomatorManager, epoch: u64 }
impl OpenPermit<'_> {
    fn check(&self) -> Result<()> {
        let state = self.manager.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown || !state.accepting || state.epoch != self.epoch { Err(CryptoError::Locked) }
        else { Ok(()) }
    }
    fn register(&self, session: &Arc<CryptomatorSession>) -> Result<()> {
        let mut state = self.manager.state.lock().map_err(|_| CryptoError::Locked)?;
        if state.shutdown || !state.accepting || state.epoch != self.epoch {
            session.request_lock();
            return Err(CryptoError::Locked);
        }
        state.sessions.push(Arc::downgrade(session));
        Ok(())
    }
}
impl Drop for OpenPermit<'_> {
    fn drop(&mut self) {
        let mut state = self.manager.state.lock().unwrap_or_else(|error| error.into_inner());
        state.pending -= 1;
    }
}
impl Drop for CryptomatorManager {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|error| error.into_inner());
        let sessions: Vec<_> = state.sessions.iter().filter_map(Weak::upgrade).collect();
        for session in &sessions { session.request_lock(); }
        for session in sessions { session.lock(); }
    }
}
