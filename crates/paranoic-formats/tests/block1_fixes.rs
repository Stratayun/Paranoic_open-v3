//! Block 1 regressions for the review fixes made after the fafd257 check:
//! leased per-file removal in cross-storage moves, manifest-bound moves,
//! version-checked copy/delete, sealed journal and retention, storage
//! withdrawal, private-directory exclusivity and cleanup, persisted password
//! backoff, immediate session revocation and KDF budget validation.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::{self, Read, Write}, rc::Rc, sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
use paranoic_core::*;
use paranoic_local::{Access, LocalStorage};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-fix-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap(); }
        Self(p)
    }
    fn dir(&self, name: &str) -> std::path::PathBuf { let p = self.0.join(name); std::fs::create_dir_all(&p).unwrap(); p }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn sid() -> StorageId { StorageId(uuid::Uuid::new_v4()) }
fn path(p: &str) -> ResourcePath { ResourcePath::parse(p).unwrap() }
fn at(storage: StorageId, p: &str) -> Location { Location { storage, path: path(p) } }
fn wait(core: &Core, id: JobId) -> JobRecord {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let job = core.job(id).unwrap();
        if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
        assert!(Instant::now() < until, "job timed out: {job:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn run(core: &Core, op: Operation) -> JobRecord { let job = core.submit(op).unwrap(); wait(core, job.id) }
fn local(dir: std::path::PathBuf) -> Arc<LocalStorage> { Arc::new(LocalStorage::open(sid(), dir, Access::ReadWrite).unwrap()) }

type Tamper = Box<dyn FnOnce() + Send>;
/// Delegating storage with test hooks: `tamper` runs once right before the
/// lease of the named source file is taken (after the copy finished); `gate`
/// blocks open_read until a message arrives.
struct Hooked { inner: Arc<LocalStorage>, tamper: Mutex<Option<(String, Tamper)>>, gate: Mutex<Option<mpsc::Receiver<()>>> }
impl Hooked {
    fn new(inner: Arc<LocalStorage>) -> Arc<Self> { Arc::new(Self { inner, tamper: Mutex::new(None), gate: Mutex::new(None) }) }
}
impl Storage for Hooked {
    fn id(&self) -> StorageId { self.inner.id() }
    fn binding_key(&self) -> Result<String> { self.inner.binding_key() }
    fn domain(&self) -> StorageDomain { self.inner.domain() }
    fn capabilities(&self) -> Capabilities { self.inner.capabilities() }
    fn metadata(&self, p: &ResourcePath) -> Result<Metadata> { self.inner.metadata(p) }
    fn list(&self, p: &ResourcePath, limit: usize) -> Result<Vec<DirectoryEntry>> { self.inner.list(p, limit) }
    fn open_read(&self, p: &ResourcePath) -> Result<Box<dyn ReadHandle>> {
        if let Some(gate) = self.gate.lock().unwrap().take() { let _ = gate.recv_timeout(Duration::from_secs(20)); }
        self.inner.open_read(p)
    }
    fn stage(&self, p: &ResourcePath, job: JobId) -> Result<Box<dyn StagedWrite>> { self.inner.stage(p, job) }
    fn create_directory(&self, p: &ResourcePath) -> Result<()> { self.inner.create_directory(p) }
    fn move_no_replace(&self, a: &ResourcePath, b: &ResourcePath) -> Result<()> { self.inner.move_no_replace(a, b) }
    fn set_directory_modified(&self, p: &ResourcePath, ms: u64) -> Result<()> { self.inner.set_directory_modified(p, ms) }
    fn delete_file(&self, p: &ResourcePath) -> Result<()> { self.inner.delete_file(p) }
    fn delete_empty_directory(&self, p: &ResourcePath) -> Result<()> { self.inner.delete_empty_directory(p) }
    fn prepare_move_source(&self, p: &ResourcePath) -> Result<Box<dyn MoveSource>> {
        let hook = { let mut slot = self.tamper.lock().unwrap();
            if slot.as_ref().is_some_and(|(name, _)| name == p.as_str()) { slot.take() } else { None } };
        if let Some((_, tamper)) = hook { tamper(); }
        self.inner.prepare_move_source(p)
    }
    fn pin_destination(&self, p: &ResourcePath) -> Result<Box<dyn PinnedDestination>> { self.inner.pin_destination(p) }
}

/// Two storages (A hooked, B plain) in one Core.
struct Pair { temp: Temp, a: Arc<Hooked>, b: Arc<LocalStorage>, core: Core }
impl Pair {
    fn new() -> Self {
        let temp = Temp::new();
        let a = Hooked::new(local(temp.dir("a")));
        let b = local(temp.dir("b"));
        let mut registry = StorageRegistry::new();
        registry.register(a.clone()).unwrap(); registry.register(b.clone()).unwrap();
        let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
        Self { temp, a, b, core }
    }
    fn a(&self, p: &str) -> Location { at(self.a.id(), p) }
    fn b(&self, p: &str) -> Location { at(self.b.id(), p) }
}

#[test] fn move_across_removes_sources_only_after_verified_copy() -> TestResult {
    let p = Pair::new();
    let src = p.temp.0.join("a/src"); std::fs::create_dir_all(src.join("sub"))?;
    std::fs::write(src.join("one"), b"first file")?; std::fs::write(src.join("sub/two"), b"second")?;
    let done = run(&p.core, Operation::MoveDirectoryAcross { source: p.a("src"), target: p.b("dst") });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert!(!src.exists(), "source tree must be removed after a verified move");
    assert_eq!(std::fs::read(p.temp.0.join("b/dst/one"))?, b"first file");
    assert_eq!(std::fs::read(p.temp.0.join("b/dst/sub/two"))?, b"second");
    Ok(())
}

#[test] fn move_across_keeps_source_whose_content_changed_after_copy() -> TestResult {
    let p = Pair::new();
    let src = p.temp.0.join("a/src"); std::fs::create_dir_all(&src)?;
    std::fs::write(src.join("doc"), b"AAAAAAAA")?;
    // Same length and restored mtime: only the content comparison can notice.
    let file = src.join("doc");
    *p.a.tamper.lock().unwrap() = Some(("src/doc".into(), Box::new(move || {
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::fs::write(&file, b"BBBBBBBB").unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(modified).unwrap();
    })));
    let done = run(&p.core, Operation::MoveDirectoryAcross { source: p.a("src"), target: p.b("dst") });
    assert_eq!(done.state, JobState::NeedsAttention, "{done:?}");
    assert_eq!(std::fs::read(src.join("doc"))?, b"BBBBBBBB", "changed source must not be removed");
    assert_eq!(std::fs::read(p.temp.0.join("b/dst/doc"))?, b"AAAAAAAA", "verified copy stays");
    Ok(())
}

#[test] fn move_across_keeps_source_whose_size_changed_after_copy() -> TestResult {
    let p = Pair::new();
    let src = p.temp.0.join("a/src"); std::fs::create_dir_all(&src)?;
    std::fs::write(src.join("doc"), b"short")?;
    let file = src.join("doc");
    *p.a.tamper.lock().unwrap() = Some(("src/doc".into(), Box::new(move || std::fs::write(&file, b"much longer now").unwrap())));
    let done = run(&p.core, Operation::MoveDirectoryAcross { source: p.a("src"), target: p.b("dst") });
    assert_eq!(done.state, JobState::NeedsAttention, "{done:?}");
    assert_eq!(std::fs::read(src.join("doc"))?, b"much longer now");
    Ok(())
}

#[test] fn manifest_bound_move_keeps_source_on_mismatch() -> TestResult {
    let p = Pair::new();
    let src = p.temp.0.join("a/src"); std::fs::create_dir_all(&src)?;
    std::fs::write(src.join("x"), b"payload")?;
    let wrong = vec![ExpectedTreeFile { path: path("x"), bytes: 7, blake3: *blake3::hash(b"PAYLOAD").as_bytes() }];
    let done = run(&p.core, Operation::MoveDirectoryExpected { source: p.a("src"), target: p.b("dst"), files: wrong });
    assert_eq!(done.state, JobState::NeedsAttention, "{done:?}");
    assert_eq!(std::fs::read(src.join("x"))?, b"payload", "source kept when the copy differs from the manifest");
    // Missing file in the manifest is a mismatch too.
    let p2 = Pair::new();
    let src2 = p2.temp.0.join("a/src"); std::fs::create_dir_all(&src2)?;
    std::fs::write(src2.join("x"), b"payload")?; std::fs::write(src2.join("extra"), b"unexpected")?;
    let exact = vec![ExpectedTreeFile { path: path("x"), bytes: 7, blake3: *blake3::hash(b"payload").as_bytes() }];
    let done = run(&p2.core, Operation::MoveDirectoryExpected { source: p2.a("src"), target: p2.b("dst"), files: exact.clone() });
    assert_eq!(done.state, JobState::NeedsAttention, "{done:?}");
    assert!(src2.join("extra").exists() && src2.join("x").exists());
    // Exact manifest: moved.
    let p3 = Pair::new();
    let src3 = p3.temp.0.join("a/src"); std::fs::create_dir_all(&src3)?;
    std::fs::write(src3.join("x"), b"payload")?;
    let done = run(&p3.core, Operation::MoveDirectoryExpected { source: p3.a("src"), target: p3.b("dst"), files: exact });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert!(!src3.exists());
    Ok(())
}

#[test] fn copy_and_delete_expected_check_versions_at_run_time() -> TestResult {
    let p = Pair::new();
    let a = p.temp.0.join("a");
    std::fs::write(a.join("src"), b"version one")?;
    let seen = p.a.metadata(&path("src"))?;
    std::fs::write(a.join("src"), b"version two!")?;
    // Source changed since planning: nothing is written.
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: seen, expected_target: None, keep_both: false, expected_source_revision: None, expected_target_revision: None });
    assert_ne!(done.state, JobState::Succeeded, "{done:?}");
    assert!(!p.temp.0.join("b/out").exists());
    // Current source, absent target: copied.
    let now = p.a.metadata(&path("src"))?;
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: now.clone(), expected_target: None, keep_both: false, expected_source_revision: None, expected_target_revision: None });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    // Target exists but was not expected: not overwritten.
    std::fs::write(p.temp.0.join("b/out"), b"user edit")?;
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: now.clone(), expected_target: None, keep_both: false, expected_source_revision: None, expected_target_revision: None });
    assert_ne!(done.state, JobState::Succeeded);
    assert_eq!(std::fs::read(p.temp.0.join("b/out"))?, b"user edit");
    // Stale expected target: not replaced.
    let stale = Metadata { bytes: 3, ..p.b.metadata(&path("out"))? };
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: now.clone(), expected_target: Some(stale), keep_both: false, expected_source_revision: None, expected_target_revision: None });
    assert_ne!(done.state, JobState::Succeeded);
    assert_eq!(std::fs::read(p.temp.0.join("b/out"))?, b"user edit");
    // keep_both: the existing file stays, a second copy appears.
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: now.clone(), expected_target: None, keep_both: true, expected_source_revision: None, expected_target_revision: None });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert_eq!(std::fs::read(p.temp.0.join("b/out"))?, b"user edit");
    assert_eq!(std::fs::read_dir(p.temp.0.join("b"))?.count(), 2);
    // Exact expected target: replaced.
    let exact = p.b.metadata(&path("out"))?;
    let done = run(&p.core, Operation::CopyFileExpected { source: p.a("src"), target: p.b("out"),
        expected_source: now, expected_target: Some(exact), keep_both: false, expected_source_revision: None, expected_target_revision: None });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert_eq!(std::fs::read(p.temp.0.join("b/out"))?, b"version two!");
    // Delete only the expected version.
    let expected = p.b.metadata(&path("out"))?;
    std::fs::write(p.temp.0.join("b/out"), b"changed again")?;
    let done = run(&p.core, Operation::DeleteFileExpected { target: p.b("out"), expected: expected.clone(), expected_revision: None });
    assert_ne!(done.state, JobState::Succeeded);
    assert!(p.temp.0.join("b/out").exists());
    let current = p.b.metadata(&path("out"))?;
    let done = run(&p.core, Operation::DeleteFileExpected { target: p.b("out"), expected: current, expected_revision: None });
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert!(!p.temp.0.join("b/out").exists());
    Ok(())
}

/// Test sealer: reversible transform with a magic prefix (not cryptography).
struct TestSealer;
impl JournalSealer for TestSealer {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        Ok([&b"SEALED1:"[..], &plaintext.iter().map(|b| b ^ 0x5a).collect::<Vec<_>>()].concat())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        let body = sealed.strip_prefix(b"SEALED1:").ok_or_else(|| CoreError::new(ErrorCode::Integrity, "not sealed"))?;
        Ok(body.iter().map(|b| b ^ 0x5a).collect())
    }
}
fn state_files_contain(dir: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().any(|e| {
        let bytes = std::fs::read(e.unwrap().path()).unwrap_or_default();
        bytes.windows(needle.len()).any(|w| w == needle)
    })
}

#[test] fn sealed_journal_keeps_names_off_disk_and_needs_its_key() -> TestResult {
    let temp = Temp::new();
    let files = local(temp.dir("files"));
    std::fs::write(temp.0.join("files/secret-name-7f3a"), b"x")?;
    let registry = || { let mut r = StorageRegistry::new(); r.register(files.clone()).unwrap(); r };
    // Plaintext journal first (older install), then the key becomes available.
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry())?;
    let done = run(&core, Operation::CopyFile { source: at(files.id(), "secret-name-7f3a"), target: at(files.id(), "copy-of-secret-7f3a"), conflict: ConflictPolicy::Fail });
    assert_eq!(done.state, JobState::Succeeded);
    // Enough history that sealing (larger records) moves rows to new pages:
    // plaintext would then survive in free pages without secure deletion.
    for i in 0..150 { assert_eq!(run(&core, Operation::CreateDirectory { target: at(files.id(), &format!("plain-dir-5e2b-{i:03}")) }).state, JobState::Succeeded); }
    core.close()?;
    assert!(state_files_contain(&temp.0.join("state"), b"secret-name-7f3a"), "precondition: plaintext journal");
    let core = Core::open_sealed(temp.0.join("state"), CoreConfig::default(), registry(), Some(Arc::new(TestSealer)))?;
    assert_eq!(core.job(done.id)?.state, JobState::Succeeded, "sealed records stay readable");
    let next = run(&core, Operation::CreateDirectory { target: at(files.id(), "another-secret-9c1d") });
    assert_eq!(next.state, JobState::Succeeded);
    core.close()?;
    assert!(!state_files_contain(&temp.0.join("state"), b"secret-name-7f3a"), "old plaintext pages removed");
    assert!(!state_files_contain(&temp.0.join("state"), b"plain-dir-5e2b"), "no plaintext left in free pages");
    assert!(!state_files_contain(&temp.0.join("state"), b"another-secret-9c1d"), "new records sealed");
    // Without the key the journal does not open (no silent empty history).
    let refused = Core::open(temp.0.join("state"), CoreConfig::default(), registry());
    assert!(matches!(refused, Err(CoreError { code: ErrorCode::PermissionDenied, .. })), "{:?}", refused.err());
    // Audit 17: after migration a plaintext row is foreign, and a sealed record
    // moved under another job id is refused.
    let db = rusqlite::Connection::open(temp.0.join("state/operations.sqlite3"))?;
    let (a, b): (String, String) = (done.id.0.to_string(), next.id.0.to_string());
    let sealed_a: Vec<u8> = db.query_row("SELECT record FROM jobs WHERE id=?1", [&a], |r| r.get(0))?;
    db.execute("UPDATE jobs SET record=?1 WHERE id=?2", rusqlite::params![sealed_a, &b])?;
    drop(db);
    let core = Core::open_sealed(temp.0.join("state"), CoreConfig::default(), registry(), Some(Arc::new(TestSealer)))?;
    assert!(matches!(core.job(next.id), Err(CoreError { code: ErrorCode::Integrity, .. })), "record accepted under another id");
    core.close()?;
    let db = rusqlite::Connection::open(temp.0.join("state/operations.sqlite3"))?;
    db.execute("UPDATE jobs SET record='{}' WHERE id=?1", [&b])?;
    drop(db);
    let reopened = Core::open_sealed(temp.0.join("state"), CoreConfig::default(), registry(), Some(Arc::new(TestSealer)));
    assert!(matches!(reopened, Err(CoreError { code: ErrorCode::Integrity, .. })), "plaintext accepted after migration: {:?}", reopened.err());
    Ok(())
}

#[test] fn retention_prunes_old_history_but_not_unresolved_attention() -> TestResult {
    let temp = Temp::new();
    let files = local(temp.dir("files"));
    let registry = || { let mut r = StorageRegistry::new(); r.register(files.clone()).unwrap(); r };
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry())?;
    let old = run(&core, Operation::CreateDirectory { target: at(files.id(), "old") });
    let attention = run(&core, Operation::CreateDirectory { target: at(files.id(), "attention") });
    let recent = run(&core, Operation::CreateDirectory { target: at(files.id(), "recent") });
    core.close()?;
    let db = rusqlite::Connection::open(temp.0.join("state/operations.sqlite3"))?;
    db.execute("UPDATE jobs SET updated_ms=0 WHERE id=?1", [old.id.0.to_string()])?;
    db.execute("UPDATE jobs SET updated_ms=0, state='needs_attention', acknowledged=0 WHERE id=?1", [attention.id.0.to_string()])?;
    drop(db);
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry())?;
    assert!(matches!(core.job(old.id), Err(CoreError { code: ErrorCode::NotFound, .. })), "old finished job pruned");
    assert!(core.job(attention.id).is_ok(), "unacknowledged attention is never pruned");
    assert!(core.job(recent.id).is_ok());
    core.close()?;
    // Retention disabled keeps everything.
    let config = CoreConfig { journal_retention_days: None, ..CoreConfig::default() };
    let core = Core::open(temp.0.join("state"), config, registry())?;
    assert!(core.job(recent.id).is_ok()); core.close()?;
    Ok(())
}

#[test] fn unregister_storage_refuses_active_jobs_and_keeps_binding() -> TestResult {
    let p = Pair::new();
    std::fs::write(p.temp.0.join("a/file"), b"data")?;
    let (release, gate) = mpsc::channel();
    *p.a.gate.lock().unwrap() = Some(gate);
    let job = p.core.submit(Operation::CopyFile { source: p.a("file"), target: p.b("file"), conflict: ConflictPolicy::Fail })?;
    let until = Instant::now() + Duration::from_secs(10);
    while p.core.job(job.id)?.state != JobState::Running { assert!(Instant::now() < until); std::thread::sleep(Duration::from_millis(5)); }
    assert!(matches!(p.core.unregister_storage(p.a.id()), Err(CoreError { code: ErrorCode::Busy, .. })), "source in use");
    assert!(matches!(p.core.unregister_storage(p.b.id()), Err(CoreError { code: ErrorCode::Busy, .. })), "target in use");
    release.send(())?;
    assert_eq!(wait(&p.core, job.id).state, JobState::Succeeded);
    p.core.unregister_storage(p.b.id())?;
    assert!(p.core.submit(Operation::CreateDirectory { target: p.b("new") }).is_err(), "withdrawn storage accepts no jobs");
    assert!(matches!(p.core.unregister_storage(p.b.id()), Err(CoreError { code: ErrorCode::NotFound, .. })));
    // A different root under the same ID is refused; the same root comes back.
    let other = Arc::new(LocalStorage::open(p.b.id(), p.temp.dir("elsewhere"), Access::ReadWrite)?);
    assert!(matches!(p.core.register_storage(other), Err(CoreError { code: ErrorCode::PermissionDenied, .. })));
    p.core.register_storage(p.b.clone())?;
    assert_eq!(run(&p.core, Operation::CreateDirectory { target: p.b("back") }).state, JobState::Succeeded);
    Ok(())
}

#[test] fn private_directory_is_exclusive_and_cleans_leftovers() -> TestResult {
    use paranoic_crypto::{CryptoError, ObjectId, PrivateDirectory};
    let temp = Temp::new();
    let first = PrivateDirectory::open(&temp.0)?;
    assert!(matches!(PrivateDirectory::open(&temp.0), Err(CryptoError::Busy)), "second owner refused");
    let object = ObjectId::generate()?;
    let mut sink = first.stage(object)?; sink.write_all(b"cached")?; sink.commit()?;
    drop(first);
    // Leftovers of an interrupted writer and an unrelated file.
    std::fs::write(temp.0.join(".pending-abandoned"), b"partial plaintext")?;
    std::fs::write(temp.0.join("keep.txt"), b"not an object")?;
    let directory = PrivateDirectory::open(&temp.0)?;
    assert!(!temp.0.join(".pending-abandoned").exists(), "interrupted temp removed on open");
    let mut data = Vec::new(); directory.open_object(object)?.read_to_end(&mut data)?; assert_eq!(data, b"cached");
    assert_eq!(directory.clear_objects()?, 1);
    assert!(directory.open_object(object).is_err());
    assert!(temp.0.join("keep.txt").exists(), "only object files are cleared");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let open = Temp::new(); std::fs::set_permissions(&open.0, std::fs::Permissions::from_mode(0o755))?;
        assert!(matches!(PrivateDirectory::open(&open.0), Err(CryptoError::Permissions)), "group/other access refused");
    }
    Ok(())
}

#[test] fn password_backoff_survives_restart() -> TestResult {
    use paranoic_crypto::{CryptoError, ObjectId, PasswordKeyring};
    let temp = Temp::new();
    let ring = PasswordKeyring::open(&temp.0)?;
    let record = ring.create(ObjectId::generate()?, b"correct horse")?;
    assert!(matches!(ring.unlock(record, b"wrong", Duration::from_secs(60)), Err(CryptoError::Authentication)));
    assert!(matches!(ring.unlock(record, b"correct horse", Duration::from_secs(60)), Err(CryptoError::RateLimited { .. })),
        "even the right password waits out the delay");
    drop(ring);
    let ring = PasswordKeyring::open(&temp.0)?;
    assert!(!ring.password_retry_after()?.is_zero(), "delay persisted across restart");
    assert!(matches!(ring.unlock(record, b"correct horse", Duration::from_secs(60)), Err(CryptoError::RateLimited { .. })));
    std::thread::sleep(ring.password_retry_after()? + Duration::from_millis(50));
    assert!(ring.unlock(record, b"correct horse", Duration::from_secs(60)).is_ok());
    Ok(())
}

#[derive(Default)] struct Output { committed: bool, aborted: bool }
struct Sink(Rc<RefCell<Output>>);
impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> { Ok(data.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl paranoic_crypto::StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().committed = true; Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().aborted = true; Ok(()) }
}

#[test] fn lock_request_stops_running_stream_and_releases_key_copy() -> TestResult {
    use paranoic_crypto::{CryptoError, KeySession, SecretKey, StreamLimits};
    let session = KeySession::new();
    session.unlock(SecretKey::generate()?, Duration::from_secs(60))?;
    let out = Rc::new(RefCell::new(Output::default()));
    let input = vec![7u8; 4 << 20];
    let mut calls = 0;
    let result = session.encrypt_stream(input.as_slice(), Box::new(Sink(out.clone())), b"ctx", StreamLimits::default(), |_| {
        calls += 1;
        assert_eq!(session.in_flight(), 1, "stream holds its own key copy");
        session.request_lock(); Ok(())
    });
    assert!(matches!(result, Err(CryptoError::Locked)), "{result:?}");
    assert_eq!(calls, 1, "no chunk processed after the lock request");
    assert!(out.borrow().aborted && !out.borrow().committed, "nothing published");
    assert_eq!(session.in_flight(), 0);
    assert!(session.wait_idle(Duration::from_millis(10)));
    assert!(matches!(session.seal(b"x", b"ctx"), Err(CryptoError::Locked)));
    Ok(())
}

#[test] fn kdf_budget_rejects_unsafe_values() {
    assert!(paranoic_formats::set_kdf_budget(0, 1 << 20).is_err());
    assert!(paranoic_formats::set_kdf_budget(17, 1 << 20).is_err());
    assert!(paranoic_formats::set_kdf_budget(2, 1024).is_err(), "below 64 MiB");
    assert!(paranoic_formats::set_kdf_budget(2, 1 << 20).is_ok());
}
