//! Lifetime of private preparation data used by journaled document jobs.
//! A lost/failed acknowledgement is never permission to delete a job's source.
use crate::*;

pub(crate) struct PublicationSpool<'a> {
    runtime: &'a NativeRuntime,
    path: ResourcePath,
    retained: bool,
}
impl<'a> PublicationSpool<'a> {
    pub(crate) fn new(runtime: &'a NativeRuntime) -> Result<Self> {
        Ok(Self { runtime, path: NativeRuntime::spool_path()?, retained: false })
    }
    pub(crate) fn path(&self) -> &ResourcePath { &self.path }
    /// Call BEFORE submitting: even an enqueue error can have a lost commit reply.
    pub(crate) fn handoff(&mut self) -> Location {
        self.retained = true;
        Location { storage: self.runtime.app.id(), path: self.path.clone() }
    }
    /// Only the completed and verified publication releases the source.
    pub(crate) fn completed(&mut self) { self.retained = false; }
}
impl Drop for PublicationSpool<'_> {
    fn drop(&mut self) {
        if !self.retained {
            if let Ok(meta) = self.runtime.app.metadata(&self.path) {
                features::remove_tree(self.runtime.app.as_ref(), &self.path, meta.kind, 0);
            }
        }
    }
}

/// Snapshot every retained journal record, not just the recent-jobs UI window.
/// If the journal cannot be read completely, perform NO cleanup. A retained
/// failure/NeedsAttention can still need its source for inspection/recovery.
pub(crate) fn referenced(core: &Core, storage: StorageId) -> paranoic_core::Result<std::collections::BTreeSet<String>> {
    let mut keep = std::collections::BTreeSet::new();
    let mut after = 0;
    loop {
        let page = core.jobs(after, None, 1000)?;
        for job in page.jobs {
            for location in job.operation.locations() {
                if location.storage != storage { continue; }
                let name = location.path.as_str().split('/').next().unwrap_or("");
                if features::is_spool(name) { keep.insert(name.to_owned()); }
            }
        }
        match page.next_cursor {
            Some(next) if next > after => after = next,
            Some(_) => return Err(paranoic_core::CoreError::new(paranoic_core::ErrorCode::Internal, "invalid spool journal cursor")),
            None => return Ok(keep),
        }
    }
}
