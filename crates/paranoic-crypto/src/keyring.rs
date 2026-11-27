use std::{fs::{File, OpenOptions}, path::Path, sync::Mutex, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{CryptoError, KeySession, ObjectId, PrivateDirectory, Result, SecretKey, WRAPPED_KEY_SIZE};

const APP_ID: i32 = 0x504b4559;
const BACKUP_MAGIC: &[u8; 8] = b"PRNBKP01";
const BACKUP_SIZE: usize = 24 + WRAPPED_KEY_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyRecord {
    pub id: ObjectId,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct KeyPage {
    pub records: Vec<KeyRecord>,
    pub next_cursor: Option<ObjectId>,
}

/// Persistent PASSWORD-wrapped keys. This is not an OS hardware keystore and
/// does not provide cryptographic erasure of SQLite pages, WAL or backups.
/// A dedicated private directory and one exclusive process lease are required.
pub struct PasswordKeyring {
    database: Mutex<Connection>,
    attempts: Mutex<AttemptLimit>,
    _lease: File,
}

impl PasswordKeyring {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let path = directory.as_ref();
        let _private = PrivateDirectory::open(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Directory contents/ancestors are exclusively host controlled, just as
        // for PrivateDirectory. Never expose this path as a general file grant.
        let lease = options.open(path.join("keyring.lock")).map_err(|_| CryptoError::KeyStore)?;
        lease.try_lock_exclusive().map_err(|_| CryptoError::Busy)?;
        let mut db = Connection::open(path.join("keyring.sqlite3")).map_err(store_error)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(store_error)?;
        let app: i32 = db.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(store_error)?;
        let version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(store_error)?;
        if !((app == 0 && version == 0) || (app == APP_ID && (version == 1 || version == 2))) {
            return Err(CryptoError::KeyStore);
        }
        if version == 0 {
            let count: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'", [], |row| row.get(0)).map_err(store_error)?;
            if count != 0 { return Err(CryptoError::KeyStore); }
        }
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0)).map_err(store_error)?;
        if !mode.eq_ignore_ascii_case("wal") { return Err(CryptoError::KeyStore); }
        db.pragma_update(None, "synchronous", "FULL").map_err(store_error)?;
        db.pragma_update(None, "secure_delete", "ON").map_err(store_error)?;
        if version == 0 {
            let tx = db.transaction().map_err(store_error)?;
            tx.execute_batch("CREATE TABLE keys (
                id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
                revision INTEGER NOT NULL CHECK(revision>0),
                wrapped BLOB NOT NULL CHECK(length(wrapped)=96)
            );").map_err(store_error)?;
            tx.pragma_update(None, "application_id", APP_ID).map_err(store_error)?;
            tx.pragma_update(None, "user_version", 1).map_err(store_error)?;
            tx.commit().map_err(store_error)?;
        }
        if version < 2 {
            // v2: failed-attempt state survives restarts (wall clock based).
            let tx = db.transaction().map_err(store_error)?;
            tx.execute_batch("CREATE TABLE attempts (
                id INTEGER PRIMARY KEY CHECK(id=1),
                failures INTEGER NOT NULL CHECK(failures BETWEEN 0 AND 6),
                last_failure_ms INTEGER NOT NULL,
                next_allowed_ms INTEGER NOT NULL
            );").map_err(store_error)?;
            tx.pragma_update(None, "user_version", 2).map_err(store_error)?;
            tx.commit().map_err(store_error)?;
        }
        let attempts = AttemptLimit::load(&db)?;
        Ok(Self { database: Mutex::new(db), attempts: Mutex::new(attempts), _lease: lease })
    }

    /// Caller persists the chosen ID before submitting, so a lost commit
    /// acknowledgement can be investigated without generating duplicate keys.
    pub fn create(&self, id: ObjectId, password: &[u8]) -> Result<KeyRecord> {
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        if load(&db, id)?.is_some() { return Err(CryptoError::AlreadyExists); }
        let key = SecretKey::generate()?;
        let wrapped = key.wrap(password, &context(id))?;
        db.execute("INSERT INTO keys(id,revision,wrapped) VALUES(?1,1,?2)", params![id.as_bytes().as_slice(), wrapped])
            .map_err(store_error)?;
        Ok(KeyRecord { id, revision: 1 })
    }

    pub fn record(&self, id: ObjectId) -> Result<KeyRecord> {
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        let (revision, _) = load(&db, id)?.ok_or(CryptoError::NotFound)?;
        Ok(KeyRecord { id, revision })
    }

    pub fn list(&self, after: Option<ObjectId>, limit: usize) -> Result<KeyPage> {
        if !(1..=256).contains(&limit) { return Err(CryptoError::InvalidInput); }
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        let mut statement = db.prepare("SELECT id,revision FROM keys WHERE (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2").map_err(store_error)?;
        let cursor = after.map(|id| id.as_bytes().to_vec());
        let rows = statement.query_map(params![cursor, limit as i64 + 1], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        }).map_err(store_error)?;
        let mut records = Vec::with_capacity(limit);
        let mut next_cursor = None;
        for row in rows {
            let (id, revision) = row.map_err(store_error)?;
            if revision <= 0 { return Err(CryptoError::KeyStore); }
            let id = ObjectId::from_bytes(id.try_into().map_err(|_| CryptoError::KeyStore)?);
            if records.len() == limit {
                next_cursor = records.last().map(|record: &KeyRecord| record.id);
                break;
            }
            records.push(KeyRecord { id, revision: revision as u64 });
        }
        Ok(KeyPage { records, next_cursor })
    }

    /// Explicit encrypted backup, never an automatic copy. The backup password
    /// is independent of the current login password. Keep it out of logs.
    pub fn export_backup(&self, expected: KeyRecord, password: &[u8], backup_password: &[u8]) -> Result<Vec<u8>> {
        crate::validate_password(backup_password)?;
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        let (revision, wrapped) = load(&db, expected.id)?.ok_or(CryptoError::NotFound)?;
        if revision != expected.revision { return Err(CryptoError::RevisionConflict); }
        let key = self.authenticate(&db, expected.id, &wrapped, password)?;
        let mut header = Vec::with_capacity(BACKUP_SIZE);
        header.extend_from_slice(BACKUP_MAGIC);
        header.extend_from_slice(expected.id.as_bytes());
        let backup = key.wrap(backup_password, &header)?;
        header.extend_from_slice(&backup);
        Ok(header)
    }

    /// Restores only into an absent ID. Existing keys are never overwritten.
    /// The caller must keep the imported ciphertext's original object contexts.
    pub fn import_backup(&self, backup: &[u8], backup_password: &[u8], new_password: &[u8]) -> Result<KeyRecord> {
        if backup.len() != BACKUP_SIZE || &backup[..8] != BACKUP_MAGIC { return Err(CryptoError::InvalidInput); }
        crate::validate_password(new_password)?;
        let id = ObjectId::from_bytes(backup[8..24].try_into().map_err(|_| CryptoError::InvalidInput)?);
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        if load(&db, id)?.is_some() { return Err(CryptoError::AlreadyExists); }
        let key = self.authenticate_context(&db, &backup[24..], backup_password, &backup[..24])?;
        let wrapped = key.wrap(new_password, &context(id))?;
        db.execute("INSERT INTO keys(id,revision,wrapped) VALUES(?1,1,?2)", params![id.as_bytes().as_slice(), wrapped]).map_err(store_error)?;
        Ok(KeyRecord { id, revision: 1 })
    }

    /// KDF work is serialized by the store mutex. Failed authentication is
    /// rate-limited in this process; execute this away from the UI thread.
    pub fn unlock(&self, expected: KeyRecord, password: &[u8], lifetime: Duration) -> Result<KeySession> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(86_400) { return Err(CryptoError::InvalidInput); }
        let db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        let (revision, wrapped) = load(&db, expected.id)?.ok_or(CryptoError::NotFound)?;
        if revision != expected.revision { return Err(CryptoError::RevisionConflict); }
        let key = self.authenticate(&db, expected.id, &wrapped, password)?;
        let session = KeySession::new();
        session.unlock(key, lifetime)?;
        Ok(session)
    }

    /// Atomic replacement of the CURRENT wrapper. The data key is unchanged.
    /// Existing sessions remain valid and must be locked by the host if needed.
    pub fn change_password(&self, expected: KeyRecord, old_password: &[u8], new_password: &[u8]) -> Result<KeyRecord> {
        let mut db = self.database.lock().map_err(|_| CryptoError::KeyStore)?;
        let (revision, wrapped) = load(&db, expected.id)?.ok_or(CryptoError::NotFound)?;
        if revision != expected.revision { return Err(CryptoError::RevisionConflict); }
        let next = revision.checked_add(1).filter(|value| *value <= i64::MAX as u64).ok_or(CryptoError::KeyStore)?;
        let key = self.authenticate(&db, expected.id, &wrapped, old_password)?;
        let replacement = key.wrap(new_password, &context(expected.id))?;
        let tx = db.transaction().map_err(store_error)?;
        let changed = tx.execute("UPDATE keys SET revision=?1,wrapped=?2 WHERE id=?3 AND revision=?4",
            params![next as i64, replacement, expected.id.as_bytes().as_slice(), revision as i64]).map_err(store_error)?;
        if changed != 1 { return Err(CryptoError::RevisionConflict); }
        tx.commit().map_err(store_error)?;
        // Push the old wrapper out of the WAL into overwritten pages
        // (secure_delete). Copies outside this database (backups, exports,
        // flash remapping) are not reached: a password change is not a
        // remedy for a compromised key — that needs key rotation.
        // Audit A18: the first column is 1 when the checkpoint was blocked; retry briefly. The new
        // password is already in force either way; a blocked checkpoint is retried at the next change.
        for attempt in 0..5u64 {
            let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0)).map_err(store_error)?;
            if busy == 0 { break; }
            std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
        }
        Ok(KeyRecord { id: expected.id, revision: next })
    }

    /// UI can query the remaining delay without waiting for a database/KDF lock.
    pub fn password_retry_after(&self) -> Result<Duration> {
        let limit = self.attempts.lock().map_err(|_| CryptoError::KeyStore)?;
        Ok(limit.remaining())
    }

    fn authenticate(&self, db: &Connection, id: ObjectId, wrapped: &[u8], password: &[u8]) -> Result<SecretKey> {
        self.authenticate_context(db, wrapped, password, &context(id))
    }

    fn authenticate_context(&self, db: &Connection, wrapped: &[u8], password: &[u8], context: &[u8]) -> Result<SecretKey> {
        let mut limit = self.attempts.lock().map_err(|_| CryptoError::KeyStore)?;
        let remaining = limit.remaining();
        if !remaining.is_zero() {
            // Round up: do not tell the host to retry earlier than admission.
            let millis = remaining.as_millis().saturating_add(1).min(u64::MAX as u128) as u64;
            return Err(CryptoError::RateLimited { retry_after_ms: millis });
        }
        if limit.last_failure.is_some_and(|last| last.elapsed() >= Duration::from_secs(300)) {
            limit.failures = 0;
        }
        // Callers hold the database mutex across authentication, serializing
        // attempts while allowing the UI to query this separate limiter.
        drop(limit);
        let result = SecretKey::unwrap(wrapped, password, context);
        if matches!(&result, Err(CryptoError::Authentication)) {
            let mut limit = self.attempts.lock().map_err(|_| CryptoError::KeyStore)?;
            limit.failures = limit.failures.saturating_add(1).min(6);
            let seconds = (1_u64 << (limit.failures - 1)).min(30);
            let now = Instant::now();
            limit.last_failure = Some(now);
            limit.next_allowed = Some(now.checked_add(Duration::from_secs(seconds)).ok_or(CryptoError::KeyStore)?);
            // Persist before reporting the failure: a restart must not reset the delay.
            limit.store(db, seconds)?;
        }
        // A successful password for another key does not reset failure history.
        result
    }
}

#[derive(Default)]
struct AttemptLimit {
    failures: u32,
    last_failure: Option<Instant>,
    next_allowed: Option<Instant>,
}

impl AttemptLimit {
    fn load(db: &Connection) -> Result<Self> {
        let row: Option<(i64, i64, i64)> = db.query_row("SELECT failures,last_failure_ms,next_allowed_ms FROM attempts WHERE id=1", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(store_error)?;
        let Some((failures, last, next)) = row else { return Ok(Self::default()) };
        let now_ms = wall_ms();
        let now = Instant::now();
        // Convert wall-clock marks to monotonic instants; a clock moved
        // backwards keeps at most the full maximum delay (30 s).
        let ago = |at: i64| Duration::from_millis(now_ms.saturating_sub(at.max(0) as u64));
        let ahead = Duration::from_millis((next.max(0) as u64).saturating_sub(now_ms)).min(Duration::from_secs(30));
        Ok(Self { failures: failures.clamp(0, 6) as u32,
            last_failure: now.checked_sub(ago(last)).or(Some(now)), next_allowed: Some(now + ahead) })
    }
    fn store(&self, db: &Connection, delay_seconds: u64) -> Result<()> {
        let now = wall_ms();
        db.execute("INSERT INTO attempts(id,failures,last_failure_ms,next_allowed_ms) VALUES(1,?1,?2,?3)
            ON CONFLICT(id) DO UPDATE SET failures=excluded.failures,last_failure_ms=excluded.last_failure_ms,next_allowed_ms=excluded.next_allowed_ms",
            params![self.failures as i64, now as i64, now.saturating_add(delay_seconds * 1000) as i64]).map_err(store_error)?;
        Ok(())
    }
    fn remaining(&self) -> Duration {
        self.next_allowed.map(|deadline| deadline.saturating_duration_since(Instant::now())).unwrap_or(Duration::ZERO)
    }
}

fn load(db: &Connection, id: ObjectId) -> Result<Option<(u64, Vec<u8>)>> {
    let row: Option<(i64, Vec<u8>)> = db.query_row("SELECT revision,wrapped FROM keys WHERE id=?1",
        [id.as_bytes().as_slice()], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(store_error)?;
    match row {
        Some((revision, wrapped)) if revision > 0 && wrapped.len() == WRAPPED_KEY_SIZE => Ok(Some((revision as u64, wrapped))),
        Some(_) => Err(CryptoError::KeyStore),
        None => Ok(None),
    }
}

fn context(id: ObjectId) -> Vec<u8> {
    let mut context = b"paranoic/password-keyring/v1/".to_vec();
    context.extend_from_slice(id.as_bytes());
    context
}

fn store_error(_: rusqlite::Error) -> CryptoError { CryptoError::KeyStore }
fn wall_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
