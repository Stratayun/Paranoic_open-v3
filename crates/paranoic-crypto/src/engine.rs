use std::{path::Path, time::Duration};

use crate::{KeyPage, KeyRecord, LifecycleEvent, ObjectId, PasswordKeyring, Result, SessionHandle, SessionManager};

/// Host entry point for the Rust crypto/key-management subblock. This owns the
/// password store and all managed sessions; raw keys never cross this boundary.
/// System key protection, UI and platform lifecycle delivery are host concerns.
pub struct CryptoEngine {
    // Drop sessions first so escaped handles are revoked before store shutdown.
    sessions: SessionManager,
    keyring: PasswordKeyring,
}

impl CryptoEngine {
    pub fn open(private_state_directory: impl AsRef<Path>, max_sessions: usize) -> Result<Self> {
        let keyring = PasswordKeyring::open(private_state_directory)?;
        let sessions = SessionManager::new(max_sessions)?;
        Ok(Self { sessions, keyring })
    }

    pub fn create_key(&self, id: ObjectId, password: &[u8]) -> Result<KeyRecord> {
        self.keyring.create(id, password)
    }

    pub fn key(&self, id: ObjectId) -> Result<KeyRecord> { self.keyring.record(id) }

    pub fn keys(&self, after: Option<ObjectId>, limit: usize) -> Result<KeyPage> {
        self.keyring.list(after, limit)
    }

    pub fn unlock(&self, record: KeyRecord, password: &[u8], lifetime: Duration) -> Result<SessionHandle> {
        self.sessions.unlock(&self.keyring, record, password, lifetime)
    }

    pub fn lock_session(&self, session: ObjectId) -> Result<()> { self.sessions.lock_session(session) }

    pub fn on_event(&self, event: LifecycleEvent) -> Result<()> { self.sessions.on_event(event) }

    /// Revoke all managed sessions BEFORE changing credentials. Remain locked
    /// on success or error; host explicitly delivers Foreground when it is ready
    /// to accept a fresh password unlock. Do not call from a stream callback.
    pub fn change_password(&self, expected: KeyRecord, old: &[u8], new: &[u8]) -> Result<KeyRecord> {
        self.sessions.while_locked(|| self.keyring.change_password(expected, old, new))
    }

    pub fn password_retry_after(&self) -> Result<Duration> { self.keyring.password_retry_after() }

    pub fn export_key(&self, record: KeyRecord, password: &[u8], backup_password: &[u8]) -> Result<Vec<u8>> {
        self.keyring.export_backup(record, password, backup_password)
    }

    pub fn import_key(&self, backup: &[u8], backup_password: &[u8], password: &[u8]) -> Result<KeyRecord> {
        self.keyring.import_backup(backup, backup_password, password)
    }
}
