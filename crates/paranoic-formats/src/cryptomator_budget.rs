//! Process-wide admission for content codecs, adapted from the legacy engine's
//! budget.rs quota/Reservation pattern. Counts work, not disk space or bytes.
use std::sync::atomic::{AtomicUsize, Ordering};
use paranoic_crypto::{CryptoError, Result};

const MAX_STREAMS: usize = 4;
static ACTIVE_STREAMS: AtomicUsize = AtomicUsize::new(0);

/// Private ownership prevents duplicated releases. Held across codec callbacks,
/// sink commit/abort and unwinding; no queue retains blocked content operations.
pub(crate) struct StreamPermit;

impl StreamPermit {
    pub(crate) fn acquire() -> Result<Self> {
        ACTIVE_STREAMS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            active.checked_add(1).filter(|next| *next <= MAX_STREAMS)
        }).map_err(|_| CryptoError::Busy)?;
        Ok(Self)
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        ACTIVE_STREAMS.fetch_sub(1, Ordering::AcqRel);
    }
}
