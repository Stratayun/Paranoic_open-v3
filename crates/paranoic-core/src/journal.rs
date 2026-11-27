use std::{fs::{File, OpenOptions}, path::Path, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};

use fs2::FileExt;
use rusqlite::{params, types::ValueRef, Connection, OptionalExtension, Transaction};

use crate::{CoreError, ErrorCode, JobId, JobPage, JobRecord, JobState, Operation, OperationResult, Result};

const APPLICATION_ID: i32 = 0x50415241;
const SCHEMA_VERSION: i32 = 2;

/// Host-provided protection of journal records (paths, names, small
/// bootstrap files). Typically a key bound to the device keystore (Android
/// Keystore, Windows DPAPI). Once a journal is sealed it cannot be opened
/// without a sealer. Ciphertext must be authenticated.
pub trait JournalSealer: Send + Sync {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>>;
    /// True when the sealing key existed before this open, in storage a file
    /// writer cannot change (Android Keystore). Then the journal has been
    /// sealed before, and plaintext without a seal marker is forged (audit 17).
    fn established(&self) -> bool { false }
}

#[derive(Clone)]
pub(crate) struct Codec { sealer: Option<Arc<dyn JournalSealer>> }

pub(crate) struct Journal {
    connection: Mutex<Connection>,
    codec: Codec,
    // The OS releases this process lease on normal exit or crash.
    _lease: File,
}

impl Journal {
    /// `directory` must be a host-owned private app directory, outside all file
    /// grants. Never use a user-selected browsing directory for this argument.
    pub fn open(directory: &Path, sealer: Option<Arc<dyn JournalSealer>>, retention_days: Option<u32>) -> Result<Self> {
        std::fs::create_dir_all(directory).map_err(CoreError::io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = std::fs::symlink_metadata(directory).map_err(CoreError::io)?;
            if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err(CoreError::new(ErrorCode::PermissionDenied, "state directory must be owned by the current user"));
            }
            // This directory is exclusively application state, never a grant.
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(CoreError::io)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lease = options.open(directory.join("core.lockfile")).map_err(CoreError::io)?;
        lease.try_lock_exclusive().map_err(|_| CoreError::new(ErrorCode::Busy, "core state is already in use"))?;
        let mut connection = Connection::open(directory.join("operations.sqlite3")).map_err(CoreError::journal)?;
        connection.busy_timeout(Duration::from_secs(5)).map_err(CoreError::journal)?;
        let application_id: i32 = connection.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(CoreError::journal)?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(CoreError::journal)?;
        if application_id != 0 && application_id != APPLICATION_ID {
            return Err(CoreError::new(ErrorCode::Journal, "state database belongs to another application"));
        }
        if !(0..=SCHEMA_VERSION).contains(&version) || (version != 0 && application_id != APPLICATION_ID) {
            return Err(CoreError::new(ErrorCode::Unsupported, "unsupported journal schema"));
        }
        if version == 0 {
            let tables: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'", [], |row| row.get(0),
            ).map_err(CoreError::journal)?;
            if tables != 0 {
                return Err(CoreError::new(ErrorCode::Journal, "refusing to initialize a nonempty unknown database"));
            }
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0)).map_err(CoreError::journal)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(CoreError::new(ErrorCode::Unsupported, "state filesystem does not support WAL"));
        }
        connection.pragma_update(None, "synchronous", "FULL").map_err(CoreError::journal)?;
        // Deleted/updated records (plaintext before sealing, pruned history) are overwritten.
        connection.pragma_update(None, "secure_delete", "ON").map_err(CoreError::journal)?;
        if version == 0 {
            let tx = connection.transaction().map_err(CoreError::journal)?;
            tx.execute_batch(
                "CREATE TABLE jobs (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    id TEXT NOT NULL UNIQUE,
                    state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled','needs_attention')),
                    record TEXT NOT NULL,
                    acknowledged INTEGER NOT NULL DEFAULT 0,
                    updated_ms INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX jobs_state_sequence ON jobs(state, sequence);
                 CREATE TABLE storage_bindings (id TEXT PRIMARY KEY, digest TEXT NOT NULL);",
            ).map_err(CoreError::journal)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID).map_err(CoreError::journal)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION).map_err(CoreError::journal)?;
            tx.commit().map_err(CoreError::journal)?;
        }
        if version == 1 {
            // v1 -> v2: filter columns that do not require reading the record.
            let tx = connection.transaction().map_err(CoreError::journal)?;
            tx.execute_batch(
                "ALTER TABLE jobs ADD COLUMN acknowledged INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE jobs ADD COLUMN updated_ms INTEGER NOT NULL DEFAULT 0;
                 UPDATE jobs SET updated_ms = COALESCE(json_extract(record,'$.updated_unix_ms'), 0),
                    acknowledged = (json_extract(record,'$.bootstrap_acceptance') IS NOT NULL
                        OR json_extract(record,'$.linked_trees_acceptance') IS NOT NULL);",
            ).map_err(CoreError::journal)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION).map_err(CoreError::journal)?;
            tx.commit().map_err(CoreError::journal)?;
        }
        let codec = Codec { sealer };
        let journal = Self { connection: Mutex::new(connection), codec, _lease: lease };
        journal.seal_plaintext()?;
        if let Some(days) = retention_days { journal.prune(days)?; }
        Ok(journal)
    }

    /// Seal records still stored as plaintext (new sealer or older journal),
    /// then rebuild the file so plaintext pages do not survive. Refuses to
    /// open a sealed journal without a sealer.
    fn seal_plaintext(&self) -> Result<()> {
        let mut connection = self.connection.lock().map_err(|_| CoreError::internal())?;
        let sealed: i64 = connection.query_row("SELECT COUNT(*) FROM jobs WHERE typeof(record)='blob'", [], |r| r.get(0)).map_err(CoreError::journal)?;
        if self.codec.sealer.is_none() {
            if sealed != 0 { return Err(CoreError::new(ErrorCode::PermissionDenied, "journal is sealed; its key is unavailable")); }
            return Ok(());
        }
        // Audit 17: once migration finished, a sealed marker records it; from
        // then on plaintext rows are foreign and refused, never re-sealed.
        connection.execute_batch("CREATE TABLE IF NOT EXISTS journal_meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);").map_err(CoreError::journal)?;
        let migrated = self.sealed_marker_valid(&connection)?;
        let plain_count: i64 = connection.query_row("SELECT COUNT(*) FROM jobs WHERE typeof(record)='text'", [], |r| r.get(0)).map_err(CoreError::journal)?;
        if migrated && plain_count != 0 {
            return Err(CoreError::new(ErrorCode::Integrity, "unexpected plaintext record in a sealed journal"));
        }
        // The marker row itself can be deleted by whoever can write the file; the
        // key's prior existence cannot. Plaintext then is not a legacy journal.
        // An empty journal (state wiped) holds nothing forged and is accepted.
        if !migrated && plain_count != 0 && self.codec.sealer.as_ref().is_some_and(|s| s.established()) {
            return Err(CoreError::new(ErrorCode::Integrity, "journal seal marker missing while its key exists"));
        }
        if migrated { return Ok(()); }
        let plain: Vec<(i64, String)> = {
            let mut statement = connection.prepare("SELECT sequence, record FROM jobs WHERE typeof(record)='text'").map_err(CoreError::journal)?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(CoreError::journal)?;
            rows.collect::<std::result::Result<_, _>>().map_err(CoreError::journal)?
        };
        let tx = connection.transaction().map_err(CoreError::journal)?;
        for (sequence, text) in &plain {
            let record = decode_json(text)?;
            tx.execute("UPDATE jobs SET record=?1 WHERE sequence=?2", params![self.codec.encode(&record)?, sequence]).map_err(CoreError::journal)?;
        }
        let marker = self.codec.sealer.as_ref().ok_or_else(CoreError::internal)?.seal(SEALED_MARKER)?;
        tx.execute("INSERT OR REPLACE INTO journal_meta(key, value) VALUES ('sealed', ?1)", params![marker]).map_err(CoreError::journal)?;
        tx.commit().map_err(CoreError::journal)?;
        if plain.is_empty() { return Ok(()); }
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);").map_err(CoreError::journal)?;
        Ok(())
    }

    fn sealed_marker_valid(&self, connection: &Connection) -> Result<bool> {
        let Some(sealer) = &self.codec.sealer else { return Ok(false) };
        let value: Option<Vec<u8>> = connection.query_row("SELECT value FROM journal_meta WHERE key='sealed'", [], |r| r.get(0))
            .optional().map_err(CoreError::journal)?;
        match value {
            None => Ok(false),
            Some(blob) => match sealer.open(&blob) {
                Ok(plain) if plain == SEALED_MARKER => Ok(true),
                _ => Err(CoreError::new(ErrorCode::Integrity, "journal seal marker failed authentication")),
            },
        }
    }

    /// Remove finished history older than `days`. Unacknowledged NeedsAttention
    /// jobs (unresolved recovery) and active jobs are never pruned.
    fn prune(&self, days: u32) -> Result<()> {
        let cutoff = now_ms()?.saturating_sub(u64::from(days) * 86_400_000);
        let cutoff = i64::try_from(cutoff).map_err(|_| CoreError::internal())?;
        self.transaction(|tx| {
            tx.execute("DELETE FROM jobs WHERE updated_ms < ?1 AND (state IN ('succeeded','failed','cancelled')
                OR (state='needs_attention' AND acknowledged=1))", [cutoff]).map_err(CoreError::journal)?;
            Ok(())
        })
    }

    /// Bind one storage; an existing binding equal to one of `legacy` digests
    /// (an earlier identity format of the same root) is replaced by `digest`.
    /// Refused while a queued or running job refers to the storage.
    pub fn bind_storage_upgrading(&self, id: crate::StorageId, digest: &str, legacy: &[String]) -> Result<()> {
        self.transaction(|tx| {
            let existing: Option<String> = tx.query_row(
                "SELECT digest FROM storage_bindings WHERE id=?1", [id.0.to_string()], |row| row.get(0),
            ).optional().map_err(CoreError::journal)?;
            match existing {
                None => { tx.execute("INSERT INTO storage_bindings(id,digest) VALUES(?1,?2)", params![id.0.to_string(), digest])
                    .map_err(CoreError::journal)?; }
                Some(existing) if existing == digest => {}
                Some(existing) if legacy.contains(&existing) => {
                    tx.execute("UPDATE storage_bindings SET digest=?2 WHERE id=?1 AND digest=?3", params![id.0.to_string(), digest, existing])
                        .map_err(CoreError::journal)?;
                }
                Some(_) => return Err(CoreError::new(ErrorCode::PermissionDenied, "storage ID cannot be rebound to another root")),
            }
            Ok(())
        })
    }
    pub fn bind_storage(&self, bindings: &[(crate::StorageId, String)]) -> Result<()> {
        self.transaction(|tx| {
            for (id, digest) in bindings {
                let existing: Option<String> = tx.query_row(
                    "SELECT digest FROM storage_bindings WHERE id=?1", [id.0.to_string()], |row| row.get(0),
                ).optional().map_err(CoreError::journal)?;
                match existing {
                    Some(existing) if existing != *digest => {
                        return Err(CoreError::new(ErrorCode::PermissionDenied, "storage ID cannot be rebound to another root"));
                    }
                    Some(_) => {},
                    None => {
                        tx.execute("INSERT INTO storage_bindings(id,digest) VALUES(?1,?2)", params![id.0.to_string(), digest])
                            .map_err(CoreError::journal)?;
                    }
                }
            }
            Ok(())
        })
    }

    /// For progress counters only: WAL with synchronous=NORMAL keeps the
    /// database consistent and orders these commits before the next FULL
    /// commit, but a power loss may drop the latest counters. Recovery-
    /// relevant transitions (effects_started, active_entry, results) use FULL.
    fn transaction_relaxed<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut connection = self.connection.lock().map_err(|_| CoreError::internal())?;
        connection.pragma_update(None, "synchronous", "NORMAL").map_err(CoreError::journal)?;
        let result = (|| {
            let tx = connection.transaction().map_err(CoreError::journal)?;
            let value = f(&tx)?;
            tx.commit().map_err(CoreError::journal)?;
            Ok(value)
        })();
        connection.pragma_update(None, "synchronous", "FULL").map_err(CoreError::journal)?;
        result
    }

    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut connection = self.connection.lock().map_err(|_| CoreError::internal())?;
        let tx = connection.transaction().map_err(CoreError::journal)?;
        let value = f(&tx)?;
        tx.commit().map_err(CoreError::journal)?;
        Ok(value)
    }

    pub fn recover(&self) -> Result<()> {
        self.transaction(|tx| {
            let records = select_state(tx, &self.codec, "running")?;
            for mut record in records {
                // Even a successful filesystem mutation may have crashed before
                // its result was committed. Never guess or replay that mutation.
                if record.effects_started {
                    record.state = JobState::NeedsAttention;
                    record.error = Some(CoreError::new(ErrorCode::NeedsAttention, "interrupted operation: inspect source, destination and staging artifact"));
                } else if record.cancel_requested {
                    record.state = JobState::Cancelled;
                } else {
                    record.state = JobState::Queued;
                }
                save(tx, &self.codec, &mut record)?;
            }
            Ok(())
        })
    }

    pub fn enqueue(&self, id: JobId, operation: Operation, capacity: usize) -> Result<JobRecord> {
        operation.validate()?;
        self.transaction(|tx| {
            if let Some(record) = find(tx, &self.codec, id)? {
                // Caller-supplied IDs make ambiguous submit acknowledgements safe.
                if encode(&record.operation)? != encode(&operation)? {
                    return Err(CoreError::new(ErrorCode::InvalidInput, "job ID was already used for a different request"));
                }
                return Ok(record);
            }
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running')", [], |row| row.get(0),
            ).map_err(CoreError::journal)?;
            if count >= capacity as i64 {
                return Err(CoreError::new(ErrorCode::Busy, "operation queue is full"));
            }
            let now = now_ms()?;
            let record = JobRecord {
                id, operation, state: JobState::Queued, bytes_done: 0, bytes_total: None,
                entries_done: 0, entries_total: None, active_entry: None,
                transfer: None, bootstrap_acceptance: None, linked_trees_acceptance: None, reviewed_unix_ms: None,
                cancel_requested: false, effects_started: false,
                created_unix_ms: now, updated_unix_ms: now, result: None, error: None,
            };
            tx.execute("INSERT INTO jobs(id,state,record,updated_ms) VALUES(?1,'queued',?2,?3)",
                params![id.0.to_string(), self.codec.encode(&record)?, now as i64]).map_err(CoreError::journal)?;
            Ok(record)
        })
    }

    pub fn get(&self, id: JobId) -> Result<JobRecord> {
        self.transaction(|tx| require(tx, &self.codec, id))
    }

    /// Newest first. Bounded snapshot polling avoids unbounded UI event queues.
    pub fn recent(&self, limit: usize) -> Result<Vec<JobRecord>> {
        if !(1..=1000).contains(&limit) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "history limit must be between 1 and 1000"));
        }
        self.transaction(|tx| {
            let mut statement = tx.prepare("SELECT id, record FROM jobs ORDER BY sequence DESC LIMIT ?1").map_err(CoreError::journal)?;
            let mut rows = statement.query([limit as i64]).map_err(CoreError::journal)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(CoreError::journal)? {
                let id: String = row.get(0).map_err(CoreError::journal)?;
                out.push(self.codec.decode_bound(&id, row.get_ref(1).map_err(CoreError::journal)?)?);
            }
            Ok(out)
        })
    }

    pub fn page(&self, after: u64, state: Option<JobState>, limit: usize) -> Result<JobPage> {
        self.page_filtered(after, state, limit, false)
    }

    pub fn unacknowledged_attention(&self, after: u64, limit: usize) -> Result<JobPage> {
        self.page_filtered(after, Some(JobState::NeedsAttention), limit, true)
    }

    fn page_filtered(&self, after: u64, state: Option<JobState>, limit: usize, unacknowledged: bool) -> Result<JobPage> {
        if after > i64::MAX as u64 || !(1..=1000).contains(&limit) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid journal page request"));
        }
        self.transaction(|tx| {
            let mut statement = tx.prepare(
                "SELECT sequence,record,id FROM jobs WHERE sequence>?1 AND (?2 IS NULL OR state=?2)
                 AND (?4=0 OR acknowledged=0)
                 ORDER BY sequence LIMIT ?3",
            ).map_err(CoreError::journal)?;
            let mut rows = statement.query(params![after as i64, state.map(state_name), limit as i64 + 1, unacknowledged]).map_err(CoreError::journal)?;
            let mut jobs = Vec::with_capacity(limit);
            let mut last = after;
            let mut has_more = false;
            while let Some(row) = rows.next().map_err(CoreError::journal)? {
                let sequence: i64 = row.get(0).map_err(CoreError::journal)?;
                if jobs.len() == limit { has_more = true; break; }
                last = u64::try_from(sequence).map_err(|_| CoreError::new(ErrorCode::Journal, "invalid journal sequence"))?;
                let id: String = row.get(2).map_err(CoreError::journal)?;
                jobs.push(self.codec.decode_bound(&id, row.get_ref(1).map_err(CoreError::journal)?)?);
            }
            Ok(JobPage { jobs, next_cursor: has_more.then_some(last) })
        })
    }

    /// Called only after a fresh readback. This transaction changes no original
    /// outcome and serializes concurrent acknowledgements into one receipt.
    pub fn accept_publication(&self, expected: &JobRecord, binding: [u8; 32],
        cancelled: &std::sync::atomic::AtomicBool) -> Result<JobRecord> {
        self.transaction(|tx| {
            let mut record = require(tx, &self.codec, expected.id)?;
            if record.operation != expected.operation || record.state != JobState::NeedsAttention
                || !record.effects_started {
                return Err(CoreError::new(ErrorCode::InvalidInput, "publication recovery request changed"));
            }
            // Decode has validated the receipt's kind, request and counts.
            // A committed decision wins over a cancelled/repeated request.
            if record.bootstrap_acceptance.is_some() || record.linked_trees_acceptance.is_some() { return Ok(record); }
            if !matches!(&record.operation, Operation::PublishTree { .. } | Operation::PublishLinkedTrees { .. }) {
                return Err(CoreError::new(ErrorCode::Unsupported, "job has no prepared publication"));
            }
            let storage = record.operation.target().storage;
            let persisted: Option<String> = tx.query_row(
                "SELECT digest FROM storage_bindings WHERE id=?1", [storage.0.to_string()], |row| row.get(0),
            ).optional().map_err(CoreError::journal)?;
            if persisted.as_deref() != Some(blake3::Hash::from(binding).to_hex().as_str()) {
                return Err(CoreError::new(ErrorCode::PermissionDenied, "publication storage binding changed"));
            }
            let recorded_unix_ms = now_ms()?;
            match &record.operation {
                Operation::PublishTree { files, .. } => {
                    record.bootstrap_acceptance = Some(crate::BootstrapAcceptance {
                        recorded_unix_ms, request_blake3: bootstrap_digest(&record.operation)?,
                        storage_binding_blake3: binding, bytes: crate::validate_prepared_tree(files)?,
                        files: files.len() as u32,
                    });
                }
                Operation::PublishLinkedTrees { data_files, files, .. } => {
                    record.linked_trees_acceptance = Some(crate::LinkedTreesAcceptance {
                        recorded_unix_ms, request_blake3: linked_digest(&record.operation)?,
                        storage_binding_blake3: binding,
                        data_bytes: crate::validate_prepared_tree(data_files)?,
                        entry_bytes: crate::validate_prepared_tree(files)?,
                        data_files: data_files.len() as u32, entry_files: files.len() as u32,
                    });
                }
                _ => return Err(CoreError::internal()),
            }
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(CoreError::new(ErrorCode::Cancelled, "publication acknowledgement cancelled"));
            }
            save(tx, &self.codec, &mut record)?;
            Ok(record)
        })
    }

    pub fn claim(&self) -> Result<Option<JobRecord>> {
        self.transaction(|tx| {
            // Audit A17: the plain `state` column only finds candidates. The sealed record decides:
            // a record that is not Queued there is never run again; its index row is rewritten from
            // the authenticated record and the next candidate is tried.
            for _ in 0..64 {
                let id: Option<String> = tx.query_row(
                    "SELECT id FROM jobs WHERE state='queued' ORDER BY sequence LIMIT 1", [], |row| row.get(0),
                ).optional().map_err(CoreError::journal)?;
                let Some(id) = id else { return Ok(None); };
                let id = JobId(uuid::Uuid::parse_str(&id).map_err(|_| CoreError::new(ErrorCode::Journal, "invalid job id"))?);
                let mut record = require(tx, &self.codec, id)?;
                if record.state != JobState::Queued {
                    save(tx, &self.codec, &mut record)?;
                    continue;
                }
                record.state = JobState::Running;
                save(tx, &self.codec, &mut record)?;
                return Ok(Some(record));
            }
            Err(CoreError::new(ErrorCode::Journal, "journal index disagrees with sealed records"))
        })
    }

    pub fn cancel(&self, id: JobId) -> Result<JobRecord> {
        self.transaction(|tx| {
            let mut record = require(tx, &self.codec, id)?;
            if matches!(record.state, JobState::Queued | JobState::Running) {
                record.cancel_requested = true;
                if record.state == JobState::Queued { record.state = JobState::Cancelled; }
                save(tx, &self.codec, &mut record)?;
            }
            Ok(record)
        })
    }

    pub fn progress(&self, id: JobId, bytes: u64, total: Option<u64>) -> Result<()> {
        self.transaction_relaxed(|tx| {
            let mut record = require_running(tx, &self.codec, id)?;
            if bytes < record.bytes_done || total.is_some_and(|n| bytes > n) {
                return Err(CoreError::new(ErrorCode::Internal, "invalid progress transition"));
            }
            record.bytes_done = bytes;
            record.bytes_total = total;
            save(tx, &self.codec, &mut record)
        })
    }

    /// Close a NeedsAttention job after the user reviewed it. Idempotent; no
    /// storage access, no change of state or error.
    pub fn mark_reviewed(&self, id: JobId) -> Result<JobRecord> {
        self.transaction(|tx| {
            let mut record = require(tx, &self.codec, id)?;
            if record.state != JobState::NeedsAttention {
                return Err(CoreError::new(ErrorCode::InvalidInput, "job does not need attention"));
            }
            if record.reviewed_unix_ms.is_none() {
                record.reviewed_unix_ms = Some(now_ms()?);
                save(tx, &self.codec, &mut record)?;
            }
            Ok(record)
        })
    }

    /// A single atomic storage call refused without changing anything: the job
    /// ends as an ordinary failure, not NeedsAttention.
    pub fn effects_unchanged(&self, id: JobId) -> Result<()> {
        self.transaction(|tx| {
            let mut record = require_running(tx, &self.codec, id)?;
            record.effects_started = false;
            save(tx, &self.codec, &mut record)
        })
    }

    pub fn begin_effects(&self, id: JobId) -> Result<()> {
        self.transaction(|tx| {
            let mut record = require_running(tx, &self.codec, id)?;
            if record.cancel_requested {
                return Err(CoreError::new(ErrorCode::Cancelled, "operation cancelled before mutation"));
            }
            record.effects_started = true;
            save(tx, &self.codec, &mut record)
        })
    }

    pub fn transfer(&self, id: JobId, receipt: crate::TransferReceipt) -> Result<()> {
        use crate::TransferPhase::*;
        self.transaction(|tx| {
            let mut record = require_running(tx, &self.codec, id)?;
            let allowed = match record.transfer.as_ref().map(|previous| previous.phase) {
                None => receipt.phase == Copying,
                Some(Copying) => receipt.phase == Publishing,
                Some(Publishing) => receipt.phase == Verifying,
                Some(Verifying) => receipt.phase == Verified,
                Some(Verified) => receipt.phase == RemovingSource,
                Some(RemovingSource) => receipt.phase == SourceRemoved,
                Some(SourceRemoved) => false,
            };
            if !allowed {
                return Err(CoreError::new(ErrorCode::Internal, "invalid transfer phase transition"));
            }
            if let Some(previous) = &record.transfer {
                if previous.phase != Copying && (previous.bytes != receipt.bytes || previous.blake3 != receipt.blake3) {
                    return Err(CoreError::new(ErrorCode::Integrity, "transfer evidence changed"));
                }
            }
            record.transfer = Some(receipt);
            save(tx, &self.codec, &mut record)
        })
    }

    pub fn entry_progress(&self, id: JobId, done: u64, total: u64, active: Option<crate::Location>) -> Result<()> {
        // The entry about to be touched must be durable before the touch;
        // pure counters (no active entry) may be relaxed.
        let durable = active.is_some();
        let body = |tx: &Transaction<'_>| {
            let mut record = require_running(tx, &self.codec, id)?;
            if done < record.entries_done || done > total
                || record.entries_total.is_some_and(|previous| previous != total)
            {
                return Err(CoreError::new(ErrorCode::Internal, "invalid entry progress transition"));
            }
            record.entries_done = done;
            record.entries_total = Some(total);
            record.active_entry = active;
            save(tx, &self.codec, &mut record)
        };
        if durable { self.transaction(body) } else { self.transaction_relaxed(body) }
    }

    pub fn finish(&self, id: JobId, result: Result<OperationResult>) -> Result<()> {
        self.transaction(|tx| {
            let mut record = require_running(tx, &self.codec, id)?;
            match result {
                Ok(result) => {
                    record.state = JobState::Succeeded;
                    record.bytes_done = result.bytes;
                    // Remote targets count staging + upload; a finished job is complete.
                    if record.bytes_total.is_some_and(|t| t > result.bytes) { record.bytes_total = Some(result.bytes); }
                    record.result = Some(result);
                    record.active_entry = None;
                }
                Err(error) => {
                    record.state = if error.code == ErrorCode::Cancelled {
                        // The executor only returns Cancelled before mutation
                        // or after an explicit, successful staged-writer abort.
                        JobState::Cancelled
                    } else if record.effects_started {
                        JobState::NeedsAttention
                    } else {
                        JobState::Failed
                    };
                    record.error = Some(error);
                }
            }
            save(tx, &self.codec, &mut record)
        })
    }
}

fn now_ms() -> Result<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| CoreError::new(ErrorCode::Internal, "system clock precedes Unix epoch"))?;
    u64::try_from(duration.as_millis()).map_err(|_| CoreError::internal())
}

fn encode(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| CoreError::new(ErrorCode::Journal, "cannot encode journal record"))
}

fn decode_json(value: &str) -> Result<JobRecord> {
    let record: JobRecord = serde_json::from_str(value)
        .map_err(|_| CoreError::new(ErrorCode::Journal, "invalid journal record"))?;
    record.operation.validate()?;
    if let Some(receipt) = &record.bootstrap_acceptance {
        let files = match &record.operation {
            Operation::PublishTree { files, .. } if record.state == JobState::NeedsAttention && record.effects_started => files,
            _ => return Err(CoreError::new(ErrorCode::Journal, "invalid bootstrap acknowledgement state")),
        };
        if receipt.request_blake3 != bootstrap_digest(&record.operation)?
            || receipt.bytes != crate::validate_prepared_tree(files)? || receipt.files as usize != files.len() {
            return Err(CoreError::new(ErrorCode::Journal, "bootstrap acknowledgement does not match request"));
        }
    }
    if let Some(receipt) = &record.linked_trees_acceptance {
        let (data_files, files) = match &record.operation {
            Operation::PublishLinkedTrees { data_files, files, .. }
                if record.state == JobState::NeedsAttention && record.effects_started && record.bootstrap_acceptance.is_none()
                => (data_files, files),
            _ => return Err(CoreError::new(ErrorCode::Journal, "invalid linked publication acknowledgement state")),
        };
        if receipt.request_blake3 != linked_digest(&record.operation)?
            || receipt.data_bytes != crate::validate_prepared_tree(data_files)?
            || receipt.entry_bytes != crate::validate_prepared_tree(files)?
            || receipt.data_files as usize != data_files.len() || receipt.entry_files as usize != files.len() {
            return Err(CoreError::new(ErrorCode::Journal, "linked acknowledgement does not match request"));
        }
    }
    Ok(record)
}

fn bootstrap_digest(operation: &Operation) -> Result<[u8; 32]> {
    let (target, files) = match operation {
        Operation::PublishTree { target, files } => (target, files),
        _ => return Err(CoreError::new(ErrorCode::Journal, "acknowledgement requires a bootstrap request")),
    };
    Ok(tree_digest(target, files))
}

fn linked_digest(operation: &Operation) -> Result<[u8; 32]> {
    let (data, data_files, target, files) = match operation {
        Operation::PublishLinkedTrees { data, data_files, target, files } => (data, data_files, target, files),
        _ => return Err(CoreError::new(ErrorCode::Journal, "acknowledgement requires linked trees")),
    };
    // Fixed-size digests in data/entry order bind both locations and their roles.
    let mut hash = blake3::Hasher::new_derive_key("paranoic.core.linked-trees-acceptance.v1");
    hash.update(&tree_digest(data, data_files));
    hash.update(&tree_digest(target, files));
    Ok(*hash.finalize().as_bytes())
}

fn tree_digest(target: &crate::Location, files: &[crate::PreparedTreeFile]) -> [u8; 32] {
    // Keep the v1 bootstrap framing stable for already persisted receipts.
    // Versioned binary framing is independent of JSON field ordering and
    // whitespace. File order is retained because it is part of the request.
    fn field(hash: &mut blake3::Hasher, bytes: &[u8]) {
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    let mut hash = blake3::Hasher::new_derive_key("paranoic.core.bootstrap-acceptance.v1");
    hash.update(target.storage.0.as_bytes());
    field(&mut hash, target.path.as_str().as_bytes());
    hash.update(&(files.len() as u64).to_be_bytes());
    for file in files {
        field(&mut hash, file.path.as_str().as_bytes());
        field(&mut hash, &file.bytes);
    }
    *hash.finalize().as_bytes()
}

fn find(tx: &Transaction<'_>, codec: &Codec, id: JobId) -> Result<Option<JobRecord>> {
    let mut statement = tx.prepare("SELECT record FROM jobs WHERE id=?1").map_err(CoreError::journal)?;
    let mut rows = statement.query([id.0.to_string()]).map_err(CoreError::journal)?;
    match rows.next().map_err(CoreError::journal)? {
        Some(row) => codec.decode_bound(&id.0.to_string(), row.get_ref(0).map_err(CoreError::journal)?).map(Some),
        None => Ok(None),
    }
}

fn require(tx: &Transaction<'_>, codec: &Codec, id: JobId) -> Result<JobRecord> {
    find(tx, codec, id)?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "job not found"))
}

fn require_running(tx: &Transaction<'_>, codec: &Codec, id: JobId) -> Result<JobRecord> {
    let record = require(tx, codec, id)?;
    if record.state != JobState::Running {
        return Err(CoreError::new(ErrorCode::Internal, "job is not running"));
    }
    Ok(record)
}

fn select_state(tx: &Transaction<'_>, codec: &Codec, state: &str) -> Result<Vec<JobRecord>> {
    let mut statement = tx.prepare("SELECT id, record FROM jobs WHERE state=?1 ORDER BY sequence").map_err(CoreError::journal)?;
    let mut rows = statement.query([state]).map_err(CoreError::journal)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(CoreError::journal)? {
        let id: String = row.get(0).map_err(CoreError::journal)?;
        out.push(codec.decode_bound(&id, row.get_ref(1).map_err(CoreError::journal)?)?);
    }
    Ok(out)
}

fn save(tx: &Transaction<'_>, codec: &Codec, record: &mut JobRecord) -> Result<()> {
    record.updated_unix_ms = now_ms()?;
    let state = state_name(record.state);
    let acknowledged = record.bootstrap_acceptance.is_some() || record.linked_trees_acceptance.is_some()
        || record.reviewed_unix_ms.is_some();
    let updated = i64::try_from(record.updated_unix_ms).map_err(|_| CoreError::internal())?;
    let changed = tx.execute("UPDATE jobs SET state=?1,record=?2,acknowledged=?3,updated_ms=?4 WHERE id=?5",
        params![state, codec.encode(record)?, acknowledged, updated, record.id.0.to_string()])
        .map_err(CoreError::journal)?;
    if changed != 1 { return Err(CoreError::new(ErrorCode::Journal, "journal transition lost its job")); }
    Ok(())
}

impl Codec {
    /// JSON text without a sealer; authenticated ciphertext BLOB with one.
    fn encode(&self, record: &JobRecord) -> Result<rusqlite::types::Value> {
        let json = encode(record)?;
        Ok(match &self.sealer {
            None => rusqlite::types::Value::Text(json),
            Some(sealer) => rusqlite::types::Value::Blob(sealer.seal(json.as_bytes())?),
        })
    }
    /// Decode and check that the record belongs to the row's job id (audit 17):
    /// a sealed record copied under another id is refused.
    fn decode_bound(&self, id: &str, value: ValueRef<'_>) -> Result<JobRecord> {
        let record = self.decode(value)?;
        if record.id.0.to_string() != id { return Err(CoreError::new(ErrorCode::Integrity, "journal record bound to another job")); }
        Ok(record)
    }
    fn decode(&self, value: ValueRef<'_>) -> Result<JobRecord> {
        match (value, &self.sealer) {
            // With a sealer every row is sealed after migration (audit 17).
            (ValueRef::Text(_), Some(_)) => Err(CoreError::new(ErrorCode::Integrity, "unexpected plaintext journal record")),
            (ValueRef::Text(text), None) => decode_json(std::str::from_utf8(text).map_err(|_| CoreError::new(ErrorCode::Journal, "invalid journal record"))?),
            (ValueRef::Blob(blob), Some(sealer)) => {
                let plain = sealer.open(blob).map_err(|_| CoreError::new(ErrorCode::Integrity, "journal record failed authentication"))?;
                decode_json(std::str::from_utf8(&plain).map_err(|_| CoreError::new(ErrorCode::Journal, "invalid journal record"))?)
            }
            (ValueRef::Blob(_), None) => Err(CoreError::new(ErrorCode::PermissionDenied, "journal is sealed; its key is unavailable")),
            _ => Err(CoreError::new(ErrorCode::Journal, "invalid journal record")),
        }
    }
}

const SEALED_MARKER: &[u8] = b"paranoic/journal/sealed-v1";

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued", JobState::Running => "running",
        JobState::Succeeded => "succeeded", JobState::Failed => "failed",
        JobState::Cancelled => "cancelled", JobState::NeedsAttention => "needs_attention",
    }
}
