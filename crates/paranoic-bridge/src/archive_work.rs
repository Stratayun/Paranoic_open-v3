//! Bounded, cancellable preparation; passwords never enter the journal.
//! Publication uses the same request ID in the durable Core queue.
use crate::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct ArchiveControl { pub(crate) cancelled: Arc<AtomicBool>, until: Instant }
impl ArchiveControl {
    pub(crate) fn new() -> Self { Self { cancelled: Arc::new(AtomicBool::new(false)), until: Instant::now() + Duration::from_secs(1800) } }
    pub(crate) fn check(&self) -> paranoic_core::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(paranoic_core::CoreError::new(paranoic_core::ErrorCode::Cancelled, "archive preparation cancelled"));
        }
        if Instant::now() >= self.until {
            return Err(paranoic_core::CoreError::new(paranoic_core::ErrorCode::LimitExceeded, "archive preparation deadline exceeded"));
        }
        Ok(())
    }
}
struct Active { id: JobId, control: ArchiveControl, started: bool }
#[derive(Default)]
pub(crate) struct ArchiveWork { active: Mutex<Option<Active>> }
pub(crate) struct ArchivePreparation<'a> { owner: &'a ArchiveWork, pub(crate) id: JobId, pub(crate) control: ArchiveControl }
impl ArchiveWork {
    fn reserve(&self, request: &str) -> Result<()> {
        let id = JobId(grant_id(request)?.0);
        let mut active = self.active.lock().map_err(lock_error)?;
        if active.is_some() { return Err(BridgeError::Busy); }
        *active = Some(Active { id, control: ArchiveControl::new(), started: false }); Ok(())
    }
    pub(crate) fn begin(&self, request: &str) -> Result<ArchivePreparation<'_>> {
        let id = JobId(grant_id(request)?.0);
        let mut active = self.active.lock().map_err(lock_error)?;
        if active.is_none() { *active = Some(Active { id, control: ArchiveControl::new(), started: false }); }
        let entry = active.as_mut().ok_or(BridgeError::Unavailable)?;
        if entry.id != id || entry.started { return Err(BridgeError::Busy); }
        entry.started = true;
        Ok(ArchivePreparation { owner: self, id, control: entry.control.clone() })
    }
    fn cancel(&self, id: JobId) -> Result<bool> {
        let active = self.active.lock().map_err(lock_error)?;
        if let Some(entry) = active.as_ref() {
            if entry.id == id { entry.control.cancelled.store(true, Ordering::Release); return Ok(true); }
        }
        Ok(false)
    }
    fn release_reserved(&self, id: JobId) {
        if let Ok(mut active) = self.active.lock() {
            if active.as_ref().is_some_and(|e| e.id == id && !e.started) { *active = None; }
        }
    }
    pub(crate) fn cancel_all(&self) {
        if let Ok(active) = self.active.lock() {
            if let Some(entry) = active.as_ref() { entry.control.cancelled.store(true, Ordering::Release); }
        }
    }
}
impl ArchivePreparation<'_> {
    /// Cancel received during submit is forwarded once the durable ID exists.
    /// Later cancellation sees no preparation and goes directly to Core.
    pub(crate) fn publication_started(&self, core: &Core) {
        let mut active = match self.owner.active.lock() { Ok(g) => g, Err(e) => e.into_inner() };
        if active.as_ref().is_some_and(|e| e.id == self.id) { *active = None; }
        let cancelled = self.control.cancelled.load(Ordering::Acquire);
        drop(active);
        if cancelled { let _ = core.cancel(self.id); }
    }
}
impl Drop for ArchivePreparation<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.owner.active.lock() {
            if active.as_ref().is_some_and(|e| e.id == self.id) { *active = None; }
        }
    }
}
#[uniffi::export]
impl NativeRuntime {
    /// The host reserves before starting a cancellable await, closing the
    /// cancellation-before-native-entry race. At most one preparation is live.
    pub fn archive_reserve_preparation(&self, request_id: String) -> Result<()> { self.archive_work.reserve(&request_id) }
    pub fn archive_release_preparation(&self, request_id: String) -> Result<()> {
        self.archive_work.release_reserved(JobId(grant_id(&request_id)?.0)); Ok(())
    }
    pub fn archive_cancel_preparation(&self, request_id: String) -> Result<()> {
        let id = JobId(grant_id(&request_id)?.0);
        if self.archive_work.cancel(id)? { return Ok(()); }
        match self.core.cancel(id) {
            Ok(_) => Ok(()),
            Err(e) if e.code == paranoic_core::ErrorCode::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    #[test] fn cancellation_is_bound_to_admission_and_request_identity() {
        let work = ArchiveWork::default();
        let first = Uuid::new_v4().to_string(); let second = Uuid::new_v4().to_string();
        work.reserve(&first).unwrap();
        assert!(!work.cancel(JobId(grant_id(&second).unwrap().0)).unwrap());
        assert!(work.cancel(JobId(grant_id(&first).unwrap().0)).unwrap());
        let preparation = work.begin(&first).unwrap();
        assert_eq!(preparation.control.check().unwrap_err().code, paranoic_core::ErrorCode::Cancelled);
        assert!(matches!(work.begin(&second), Err(BridgeError::Busy)));
        drop(preparation);
        assert!(work.begin(&second).is_ok());
    }
}

