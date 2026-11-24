//! Process-wide admission for expensive password derivations (Argon2id of
//! format 1.8, scrypt of Cryptomator). Bounds concurrent derivations and the
//! total memory in flight, so several opens cannot exhaust a phone's memory. Waiting is
//! cancellable through the caller's checkpoint; a running derivation itself
//! is not interruptible (one KDF call).
use std::{sync::{Condvar, Mutex}, time::Duration};
use paranoic_crypto::{CryptoError, Result};

struct State { running: u32, memory_kib: u64, max_running: u32, max_memory_kib: u64 }
static STATE: Mutex<State> = Mutex::new(State { running: 0, memory_kib: 0,
    max_running: if cfg!(target_os = "android") { 1 } else { 2 },
    // Imported header costs are unauthenticated until after Argon2. Do not
    // let a hostile vault allocate a desktop-sized 1 GiB on a phone.
    max_memory_kib: if cfg!(target_os = "android") { 256 * 1024 } else { 1024 * 1024 },
});
static WAKE: Condvar = Condvar::new();

/// Host policy: e.g. (1, 256 MiB) on low-memory phones, (2, 1 GiB) on desktops.
pub fn set_kdf_budget(max_running: u32, max_memory_kib: u64) -> Result<()> {
    if !(1..=16).contains(&max_running) || max_memory_kib < 64 * 1024 { return Err(CryptoError::InvalidInput); }
    let mut state = STATE.lock().map_err(|_| CryptoError::Busy)?;
    state.max_running = max_running; state.max_memory_kib = max_memory_kib;
    WAKE.notify_all();
    Ok(())
}

pub(crate) struct Permit { memory_kib: u64 }
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = match STATE.lock() { Ok(s) => s, Err(e) => e.into_inner() };
        state.running = state.running.saturating_sub(1);
        state.memory_kib = state.memory_kib.saturating_sub(self.memory_kib);
        WAKE.notify_all();
    }
}

/// Wait for a slot with `memory_kib` of Argon2 budget (0 for PBKDF2).
/// A request larger than the whole budget fails instead of waiting forever.
pub(crate) fn admit(memory_kib: u64, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Permit> {
    let mut state = STATE.lock().map_err(|_| CryptoError::Busy)?;
    loop {
        if memory_kib > state.max_memory_kib { return Err(CryptoError::LimitExceeded); }
        if state.running < state.max_running && state.memory_kib + memory_kib <= state.max_memory_kib {
            state.running += 1; state.memory_kib += memory_kib;
            return Ok(Permit { memory_kib });
        }
        drop(state);
        checkpoint()?;
        state = STATE.lock().map_err(|_| CryptoError::Busy)?;
        state = WAKE.wait_timeout(state, Duration::from_millis(100)).map_err(|_| CryptoError::Busy)?.0;
    }
}
