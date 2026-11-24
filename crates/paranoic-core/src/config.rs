use crate::{CoreError, ErrorCode, Result};

/// Explicit resource bounds. A single worker serializes mutations per Core.
#[derive(Clone, Debug)]
pub struct CoreConfig {
    pub copy_buffer_bytes: usize,
    pub progress_interval_bytes: u64,
    pub max_file_bytes: u64,
    pub max_queued_jobs: usize,
    pub max_list_entries: usize,
    /// Finished job history older than this is removed at open (unresolved
    /// NeedsAttention jobs are kept). None keeps history forever.
    pub journal_retention_days: Option<u32>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            copy_buffer_bytes: 1024 * 1024,
            progress_interval_bytes: 8 * 1024 * 1024,
            max_file_bytes: 1024 * 1024 * 1024 * 1024,
            max_queued_jobs: 1024,
            max_list_entries: 10_000,
            journal_retention_days: Some(30),
        }
    }
}

impl CoreConfig {
    pub fn validate(&self) -> Result<()> {
        if !(64 * 1024..=8 * 1024 * 1024).contains(&self.copy_buffer_bytes)
            || self.progress_interval_bytes < self.copy_buffer_bytes as u64
            || self.max_file_bytes == 0
            || self.max_file_bytes > i64::MAX as u64
            || !(1..=100_000).contains(&self.max_queued_jobs)
            || !(1..=100_000).contains(&self.max_list_entries)
            || self.journal_retention_days.is_some_and(|d| !(1..=3650).contains(&d))
        {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid core resource limits"));
        }
        Ok(())
    }
}
