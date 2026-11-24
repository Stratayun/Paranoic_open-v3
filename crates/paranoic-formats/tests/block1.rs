//! Block 1 integration regression suite for the Linux native adapter.
//! Run KDF-heavy tests serially.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, rc::Rc, io::{self, Write}, sync::Arc, time::{Duration, Instant}};
use paranoic_core::*;
use paranoic_crypto::{CryptoError, SecretKey, StreamSink, StreamLimits, KeySession};
use paranoic_formats::*;
use paranoic_local::{LocalStorage, Access};
use zeroize::Zeroizing;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
#[derive(Default)] struct Output { bytes: Vec<u8>, committed: bool, aborted: bool }
struct Sink { pending: Vec<u8>, output: Rc<RefCell<Output>> }
impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> { self.pending.extend_from_slice(data); Ok(data.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        let mut out = self.output.borrow_mut(); out.bytes = std::mem::take(&mut self.pending); out.committed = true; Ok(())
    }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.output.borrow_mut().aborted = true; Ok(()) }
}
fn sink() -> (Box<dyn StreamSink>, Rc<RefCell<Output>>) {
    let out = Rc::new(RefCell::new(Output::default()));
    (Box::new(Sink { pending: vec![], output: out.clone() }), out)
}
fn no_publication(out: &Rc<RefCell<Output>>) { let o = out.borrow(); assert!(o.aborted); assert!(!o.committed); assert!(o.bytes.is_empty()); }
fn password() -> Zeroizing<String> { Zeroizing::new("integration-test-password".into()) }
fn id() -> JobId { JobId(uuid::Uuid::new_v4()) }
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap(); Self(p)
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
struct Harness { core: Core, storage: Arc<LocalStorage>, temp: Temp }
impl Harness {
    fn new() -> Self {
        let temp = Temp::new(); let files = temp.0.join("files"); std::fs::create_dir(&files).unwrap();
        let storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), files, Access::ReadWrite).unwrap());
        let mut registry = StorageRegistry::new(); registry.register(storage.clone()).unwrap();
        let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
        Self { core, storage, temp }
    }
    fn loc(&self, path: &str) -> Location { Location { storage: self.storage.id(), path: ResourcePath::parse(path).unwrap() } }
    fn wait(&self, id: JobId) -> JobRecord {
        let until = Instant::now() + Duration::from_secs(10);
        loop { let job = self.core.job(id).unwrap();
            if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
            assert!(Instant::now() < until, "job timed out"); std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn run(&self, op: Operation) -> JobRecord {
        let job = self.core.submit(op).unwrap(); let result = self.wait(job.id);
        assert_eq!(result.state, JobState::Succeeded, "{result:?}"); result
    }
    fn vault(&self) -> CryptomatorReader {
        let bootstrap = CryptomatorVault::prepare_create(password(), || Ok(())).unwrap();
        let job = bootstrap.submit(&self.core, self.loc("vault"), id()).unwrap();
        let completed = self.wait(job.id); assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
        CryptomatorReader::open(self.storage.clone(), ResourcePath::parse("vault").unwrap(), password(), Duration::from_secs(300), || Ok(())).unwrap()
    }
    fn mkdir(&self, reader: &CryptomatorReader, parent: &[String], name: &str) {
        let prepared = reader.prepare_directory(parent, name, || Ok(())).unwrap();
        let job = reader.submit_directory(&self.core, &prepared, id()).unwrap();
        let completed = self.wait(job.id); assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    }
}

#[test] fn paths_reject_escape_and_reserved_names() {
    for path in ["../secret", "/absolute", "a//b", "a/../b", "C:foo", "CON", "LPT1.txt", "a\\b", ".paranoic-job", "tail."] {
        assert!(ResourcePath::parse(path).is_err(), "{path}");
    }
    assert!(serde_json::from_str::<ResourcePath>("\"../secret\"").is_err());
    assert!(ResourcePath::parse("folder/данные.txt").is_ok());
}
#[test] fn core_copy_conflict_move_delete_and_history() -> TestResult {
    let h = Harness::new();
    std::fs::write(h.temp.0.join("files/source"), b"original")?;
    let operation = Operation::CopyVerified { source: h.loc("source"), target: h.loc("copy") };
    let job_id = id(); h.core.submit_with_id(job_id, operation.clone())?;
    let completed = h.wait(job_id); assert_eq!(completed.state, JobState::Succeeded, "{completed:?}");
    assert_eq!(h.core.submit_with_id(job_id, operation)?.id, job_id);
    assert!(h.core.submit_with_id(job_id, Operation::CreateFile { target: h.loc("other") }).is_err());
    let conflict = h.core.submit(Operation::CopyFile { source: h.loc("source"), target: h.loc("copy"), conflict: ConflictPolicy::Fail })?;
    assert_ne!(h.wait(conflict.id).state, JobState::Succeeded);
    assert_eq!(std::fs::read(h.temp.0.join("files/copy"))?, b"original");
    h.run(Operation::Move { source: h.loc("copy"), target: h.loc("moved") });
    h.run(Operation::DeleteFile { target: h.loc("moved") });
    assert!(!h.temp.0.join("files/moved").exists());
    let mut registry = StorageRegistry::new(); registry.register(h.storage.clone())?;
    h.core.close()?;
    let reopened = Core::open(h.temp.0.join("state"), CoreConfig::default(), registry)?;
    assert_eq!(reopened.job(job_id)?.state, JobState::Succeeded); reopened.close()?; Ok(())
}
#[test] fn local_readonly_symlink_and_atomic_conflict() -> TestResult {
    let h = Harness::new();
    std::fs::write(h.temp.0.join("outside"), b"private")?;
    #[cfg(unix)] { std::os::unix::fs::symlink(h.temp.0.join("outside"), h.temp.0.join("files/link"))?;
        assert!(h.storage.open_read(&ResourcePath::parse("link")?).is_err()); }
    let ro = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), h.temp.0.join("files"), Access::ReadOnly)?;
    assert!(ro.stage(&ResourcePath::parse("new")?, id()).is_err());
    let path = ResourcePath::parse("target")?;
    let mut staged = h.storage.stage(&path, id())?; staged.write_all(b"new")?;
    std::fs::write(h.temp.0.join("files/target"), b"keep")?;
    assert!(staged.publish().is_err()); assert_eq!(std::fs::read(h.temp.0.join("files/target"))?, b"keep"); Ok(())
}
#[test] fn key_context_password_and_lock() -> TestResult {
    let key = SecretKey::generate()?; let data = key.seal(b"payload", b"context")?;
    assert_eq!(&*key.open(&data, b"context")?, b"payload"); assert!(key.open(&data, b"other").is_err());
    let wrapped = key.wrap(b"password", b"key-id")?;
    assert!(SecretKey::unwrap(&wrapped, b"wrong", b"key-id").is_err());
    let restored = SecretKey::unwrap(&wrapped, b"password", b"key-id")?;
    assert_eq!(&*restored.open(&data, b"context")?, b"payload");
    let session = KeySession::new(); session.unlock(key, Duration::from_secs(60))?; session.request_lock();
    assert!(matches!(session.seal(b"x", b"context"), Err(CryptoError::Locked))); Ok(())
}
#[test] fn secret_stream_boundary_and_corruption() -> TestResult {
    let key = SecretKey::generate()?;
    for size in [0, 1, 1024 * 1024, 1024 * 1024 + 1] {
        let input = vec![42; size]; let (s, encrypted) = sink();
        key.encrypt_stream(input.as_slice(), s, b"file", StreamLimits::default(), |_| Ok(()))?;
        let bytes = encrypted.borrow().bytes.clone(); let (s, plain) = sink();
        key.decrypt_stream(bytes.as_slice(), s, b"file", StreamLimits::default(), |_| Ok(()))?;
        assert_eq!(plain.borrow().bytes, input);
        for bad in [bytes[..bytes.len()-1].to_vec(), [bytes.as_slice(), &[0]].concat()] {
            let (s, out) = sink(); assert!(key.decrypt_stream(bad.as_slice(), s, b"file", StreamLimits::default(), |_| Ok(())).is_err()); no_publication(&out);
        }
    } Ok(())
}
#[test] fn age_roundtrip_wrong_identity_tamper_and_limit() -> TestResult {
    let identity = AgeIdentity::generate(); let (s, encrypted) = sink(); let data = vec![17; 65537];
    AgeFormat::encrypt_recipients(data.as_slice(), s, &[identity.recipient()], AgeLimits::default(), |_| Ok(()))?;
    let bytes = encrypted.borrow().bytes.clone(); let (s, plain) = sink();
    AgeFormat::decrypt_identity(bytes.as_slice(), s, &identity, AgeLimits::default(), |_| Ok(()))?;
    assert_eq!(plain.borrow().bytes, data);
    let (s, out) = sink(); assert!(AgeFormat::decrypt_identity(bytes.as_slice(), s, &AgeIdentity::generate(), AgeLimits::default(), |_| Ok(())).is_err()); no_publication(&out);
    let mut bad = bytes; *bad.last_mut().unwrap() ^= 1; let (s, out) = sink();
    assert!(AgeFormat::decrypt_identity(bad.as_slice(), s, &identity, AgeLimits::default(), |_| Ok(())).is_err()); no_publication(&out);
    let (s, out) = sink(); assert!(AgeFormat::encrypt_recipients(data.as_slice(), s, &[identity.recipient()], AgeLimits { max_plaintext_bytes: 10, ..Default::default() }, |_| Ok(())).is_err()); no_publication(&out); Ok(())
}
#[test] fn cryptomator_chunks_trusted_length_and_authentication() -> TestResult {
    let key = CryptomatorGcmKey::from_unwrapped_encryption_key(Zeroizing::new([7;32]));
    for size in [0, 1, 32768, 32769] {
        let data = vec![91;size]; let (s, encrypted) = sink(); key.encrypt(data.as_slice(), s, Default::default(), |_| Ok(()))?;
        let bytes = encrypted.borrow().bytes.clone(); let (s, plain) = sink();
        key.decrypt(bytes.as_slice(), s, Default::default(), CryptomatorLengthPolicy::RequireTrusted(size as u64), |_| Ok(()))?;
        assert_eq!(plain.borrow().bytes, data);
        let (s, out) = sink(); assert!(key.decrypt(bytes.as_slice(), s, Default::default(), CryptomatorLengthPolicy::RequireTrusted(size as u64 + 1), |_| Ok(())).is_err()); no_publication(&out);
        let mut bad = bytes; *bad.last_mut().unwrap() ^= 1; let (s, out) = sink();
        assert!(key.decrypt(bad.as_slice(), s, Default::default(), CryptomatorLengthPolicy::AllowUnauthenticated, |_| Ok(())).is_err()); no_publication(&out);
    } Ok(())
}
#[test] fn cryptomator_create_import_move_and_cycle_guard() -> TestResult {
    let h = Harness::new(); let reader = h.vault(); h.mkdir(&reader, &[], "folder");
    // Exercise both ordinary files and shortened wrappers across parents.
    for name in ["short".to_owned(), "x".repeat(180)] {
        let (s, spool) = sink(); let prepared = reader.prepare_file_import(&[], &name, &b"file data"[..], s, Default::default(), |_| Ok(()))?;
        std::fs::write(h.temp.0.join("files/spool"), &spool.borrow().bytes)?;
        let job = reader.submit_file_import(&h.core, &prepared, h.loc("spool"), id())?; assert_eq!(h.wait(job.id).state, JobState::Succeeded);
        let target = format!("{name}2"); let plan = reader.prepare_move(&[], &name, &["folder".into()], &target, || Ok(()))?;
        let job = reader.submit_rename(&h.core, &plan, id())?; assert_eq!(h.wait(job.id).state, JobState::Succeeded);
        assert_eq!(reader.submit_rename(&h.core, &plan, job.id)?.id, job.id);
        let (s, out) = sink(); reader.decrypt_file(&["folder".into(), target], s, Default::default(), CryptomatorLengthPolicy::RequireTrusted(9), |_| Ok(()))?;
        assert_eq!(out.borrow().bytes, b"file data");
    }
    h.mkdir(&reader, &["folder".into()], "child");
    h.mkdir(&reader, &[], "sibling");
    // Deeper directory moves are supported after a bounded subtree depth walk.
    let deeper = reader.prepare_move(&[], "sibling", &["folder".into()], "deeper", || Ok(()))?;
    let job = reader.submit_rename(&h.core, &deeper, id())?; assert_eq!(h.wait(job.id).state, JobState::Succeeded);
    assert!(reader.list_directory(&["folder".into(), "deeper".into()], 100, || Ok(())).is_ok());
    assert!(matches!(reader.prepare_move(&[], "folder", &["folder".into()], "self", || Ok(())), Err(CryptoError::InvalidInput)));
    assert!(matches!(reader.prepare_move(&[], "folder", &["folder".into(), "child".into()], "cycle", || Ok(())), Err(CryptoError::InvalidInput)));
    let plan = reader.prepare_move(&["folder".into()], "child", &[], "moved-child", || Ok(()))?;
    let job = reader.submit_rename(&h.core, &plan, id())?; assert_eq!(h.wait(job.id).state, JobState::Succeeded);
    assert!(reader.list_directory(&["moved-child".into()], 100, || Ok(())).is_ok());
    reader.request_lock(); assert!(reader.prepare_move(&[], "folder", &[], "locked", || Ok(())).is_err()); Ok(())
}
#[test] fn cancellation_aborts_output() -> TestResult {
    let key = SecretKey::generate()?; let (s, out) = sink();
    assert!(matches!(key.encrypt_stream(&b"secret"[..], s, b"context", Default::default(), |_| Err(CryptoError::Cancelled)), Err(CryptoError::Cancelled))); no_publication(&out);
    let key = CryptomatorGcmKey::from_unwrapped_encryption_key(Zeroizing::new([9;32])); let (s, out) = sink();
    assert!(matches!(key.encrypt(&b"secret"[..], s, Default::default(), |_| Err(CryptoError::Cancelled)), Err(CryptoError::Cancelled))); no_publication(&out); Ok(())
}

#[test] fn bootstrap_inspection_detects_mutation_and_cancellation() -> TestResult {
    use std::sync::atomic::AtomicBool;
    let h = Harness::new();
    let job = h.run(Operation::PublishTree { target: h.loc("tree"), files: vec![PreparedTreeFile { path: ResourcePath::parse("nested/file")?, bytes: b"wrapped ciphertext".to_vec() }] });
    assert_eq!(h.core.inspect_prepared_tree(job.id, &AtomicBool::new(false))?, PreparedTreeObservation::Matches);
    assert!(h.core.inspect_prepared_tree(job.id, &AtomicBool::new(true)).is_err());
    std::fs::write(h.temp.0.join("files/tree/nested/file"), b"changed ciphertext")?;
    assert!(matches!(h.core.inspect_prepared_tree(job.id, &AtomicBool::new(false))?, PreparedTreeObservation::Differs { .. }));
    assert_eq!(h.core.job(job.id)?.state, JobState::Succeeded); Ok(())
}
#[test] fn grant_cannot_be_rebound_and_core_has_exclusive_lease() -> TestResult {
    let h = Harness::new(); let mut registry = StorageRegistry::new(); registry.register(h.storage.clone())?;
    assert!(matches!(Core::open(h.temp.0.join("state"), Default::default(), registry), Err(CoreError { code: ErrorCode::Busy, .. })));
    h.core.close()?;
    std::fs::create_dir(h.temp.0.join("different"))?;
    let mut registry = StorageRegistry::new();
    registry.register(Arc::new(LocalStorage::open(h.storage.id(), h.temp.0.join("different"), Access::ReadWrite)?))?;
    assert!(matches!(Core::open(h.temp.0.join("state"), Default::default(), registry), Err(CoreError { code: ErrorCode::PermissionDenied, .. }))); Ok(())
}
#[test] fn keyring_password_revision_and_background_revocation() -> TestResult {
    use paranoic_crypto::{PasswordKeyring, SessionManager, LifecycleEvent, ObjectId};
    let temp = Temp::new(); let key_dir = temp.0.join("keys"); std::fs::create_dir(&key_dir)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))?; }
    let ring = PasswordKeyring::open(&key_dir)?;
    let record = ring.create(ObjectId::generate()?, b"old password")?;
    let manager = SessionManager::new(2)?;
    let session = manager.unlock(&ring, record, b"old password", Duration::from_secs(60))?;
    let ciphertext = session.seal(b"sensitive", b"context")?;
    manager.on_event(LifecycleEvent::Background)?;
    assert!(session.open(&ciphertext, b"context").is_err());
    assert!(manager.unlock(&ring, record, b"old password", Duration::from_secs(60)).is_err());
    manager.on_event(LifecycleEvent::Foreground)?;
    let changed = ring.change_password(record, b"old password", b"new password")?;
    assert!(matches!(ring.unlock(record, b"old password", Duration::from_secs(60)), Err(CryptoError::RevisionConflict)));
    let restored = ring.unlock(changed, b"new password", Duration::from_secs(60))?;
    assert_eq!(&*restored.open(&ciphertext, b"context")?, b"sensitive");
    manager.on_event(LifecycleEvent::Shutdown)?; assert!(matches!(manager.on_event(LifecycleEvent::Foreground), Err(CryptoError::Locked)));
    assert!(manager.unlock(&ring, changed, b"new password", Duration::from_secs(60)).is_err()); Ok(())
}
#[test] fn cryptomator_names_bound_to_parent_and_mapping() -> TestResult {
    let key = CryptomatorNameKey::from_unwrapped_master_key(Zeroizing::new([3;64]));
    let root = CryptomatorDirectoryId::root(); let other = CryptomatorDirectoryId::generate()?;
    let name = key.encrypt_name(&root, "документ.txt")?;
    assert_eq!(&*key.decrypt_name(&root, &name)?, "документ.txt");
    assert!(key.decrypt_name(&other, &name).is_err());
    assert_ne!(key.directory_path(&root)?, key.directory_path(&other)?);
    for name in ["", ".", "..", "a/b", "a\\b"] { assert!(key.encrypt_name(&root, name).is_err()); } Ok(())
}
#[test] fn mapped_move_conflict_retains_original_and_rejects_descendant() -> TestResult {
    let h = Harness::new(); let root = h.temp.0.join("files");
    std::fs::create_dir_all(root.join("source"))?; std::fs::create_dir_all(root.join("other/target"))?;
    std::fs::write(root.join("source/name.c9s"), b"old.c9r")?;
    std::fs::write(root.join("source/contents.c9r"), b"ciphertext")?;
    assert!(h.storage.rename_mapped(&ResourcePath::parse("source")?, &ResourcePath::parse("other/target")?, id(), b"old.c9r", b"new.c9r").is_err());
    assert_eq!(std::fs::read(root.join("source/contents.c9r"))?, b"ciphertext");
    assert!(Operation::RenameMapped { source: h.loc("source"), target: h.loc("source/child"), old_mapping: b"old".to_vec(), new_mapping: b"new".to_vec() }.validate().is_err()); Ok(())
}

#[test] fn recovery_preserves_uncertain_effects_and_accepts_only_fresh_matches() -> TestResult {
    use std::sync::atomic::AtomicBool;
    let h = Harness::new();
    let mut job = h.run(Operation::PublishTree { target: h.loc("published"), files: vec![PreparedTreeFile { path: ResourcePath::parse("file")?, bytes: b"ciphertext".to_vec() }] });
    let mut registry = StorageRegistry::new(); registry.register(h.storage.clone())?;
    h.core.close()?;
    // Fault injection: persisted Running + effects_started is the state left
    // by interruption after publication and before final journal completion.
    job.state = JobState::Running; job.result = None; job.error = None;
    let db = rusqlite::Connection::open(h.temp.0.join("state/operations.sqlite3"))?;
    db.execute("UPDATE jobs SET state='running', record=?1 WHERE id=?2", rusqlite::params![serde_json::to_string(&job)?, job.id.0.to_string()])?; drop(db);
    let core = Core::open(h.temp.0.join("state"), Default::default(), registry)?;
    assert_eq!(core.job(job.id)?.state, JobState::NeedsAttention);
    std::fs::write(h.temp.0.join("files/published/file"), b"tampered")?;
    assert!(core.accept_prepared_tree(job.id, &AtomicBool::new(false)).is_err());
    assert!(core.job(job.id)?.bootstrap_acceptance.is_none());
    std::fs::write(h.temp.0.join("files/published/file"), b"ciphertext")?;
    core.accept_prepared_tree(job.id, &AtomicBool::new(false))?;
    let accepted = core.job(job.id)?; assert_eq!(accepted.state, JobState::NeedsAttention); assert!(accepted.bootstrap_acceptance.is_some());
    std::fs::write(h.temp.0.join("files/published/file"), b"later edit")?;
    core.accept_prepared_tree(job.id, &AtomicBool::new(false))?;
    assert_eq!(core.job(job.id)?.bootstrap_acceptance, accepted.bootstrap_acceptance);
    assert!(matches!(core.inspect_prepared_tree(job.id, &AtomicBool::new(false))?, PreparedTreeObservation::Differs { .. })); core.close()?; Ok(())
}
#[test] fn expected_copy_rejects_corrupt_spool_without_publication() -> TestResult {
    let h = Harness::new(); std::fs::write(h.temp.0.join("files/spool"), b"wrong")?;
    let job = h.core.submit(Operation::CopyExpected { source: h.loc("spool"), target: h.loc("target"), bytes: 5, blake3: *blake3::hash(b"right").as_bytes() })?;
    assert_ne!(h.wait(job.id).state, JobState::Succeeded);
    assert!(!h.temp.0.join("files/target").exists()); assert_eq!(std::fs::read(h.temp.0.join("files/spool"))?, b"wrong"); Ok(())
}

#[test] fn private_sink_commits_and_preserves_existing_object() -> TestResult {
    use paranoic_crypto::{PrivateDirectory, ObjectId};
    use std::io::Read;
    let temp = Temp::new();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&temp.0, std::fs::Permissions::from_mode(0o700))?; }
    let directory = PrivateDirectory::open(&temp.0)?; let object = ObjectId::generate()?;
    let mut writer = directory.stage(object)?; writer.write_all(b"private contents")?; writer.commit()?;
    let mut duplicate = directory.stage(object)?; duplicate.write_all(b"replacement")?; assert!(duplicate.commit().is_err());
    let mut data = Vec::new(); directory.open_object(object)?.read_to_end(&mut data)?; assert_eq!(data, b"private contents");
    directory.remove_object(object)?; assert!(directory.open_object(object).is_err()); Ok(())
}
