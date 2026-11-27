//! Block 1 verification: Cryptomator vaults written by Cryptomator's own
//! libraries (tools/interop/cryptomator) and Core 1.2 operations.
//! Cryptomator interop runs only when PARANOIC_CRYPTOMATOR_FIXTURES points to a
//! directory produced by `interop.Main create`; afterwards run `interop.Main
//! verify` on the same directory to check what Paranoic wrote.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::{self, Write}, path::PathBuf, rc::Rc, sync::Arc, time::{Duration, Instant}};
use paranoic_core::*;
use paranoic_crypto::StreamSink;
use paranoic_formats::*;
use paranoic_local::{Access, LocalStorage};
use zeroize::Zeroizing;

// Derivations share the process-wide KDF budget (paranoic_crypto::kdf_gate);
// these interop users stay serial so their timing does not depend on it.
static KDF_INTEROP: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Default)] struct Output { bytes: Vec<u8>, committed: bool }
struct Sink { pending: Vec<u8>, output: Rc<RefCell<Output>> }
impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> { self.pending.extend_from_slice(data); Ok(data.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        let mut out = self.output.borrow_mut(); out.bytes = std::mem::take(&mut self.pending); out.committed = true; Ok(())
    }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}
fn sink() -> (Box<dyn StreamSink>, Rc<RefCell<Output>>) {
    let out = Rc::new(RefCell::new(Output::default()));
    (Box::new(Sink { pending: vec![], output: out.clone() }), out)
}
fn id() -> JobId { JobId(uuid::Uuid::new_v4()) }
fn s(v: &str) -> String { v.to_owned() }
fn pattern(len: usize, seed: u8) -> Vec<u8> { (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-interop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap(); Self(p)
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

struct Harness { core: Core, storages: Vec<Arc<LocalStorage>>, _state: Temp }
impl Harness {
    /// One Core with a LocalStorage grant per root directory.
    fn new(roots: &[PathBuf]) -> Self {
        let state = Temp::new();
        let mut registry = StorageRegistry::new();
        let mut storages = Vec::new();
        for root in roots {
            let storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), root, Access::ReadWrite).unwrap());
            registry.register(storage.clone()).unwrap();
            storages.push(storage);
        }
        let core = Core::open(state.0.join("state"), CoreConfig::default(), registry).unwrap();
        Self { core, storages, _state: state }
    }
    fn loc(&self, storage: usize, path: &str) -> Location {
        Location { storage: self.storages[storage].id(), path: ResourcePath::parse(path).unwrap() }
    }
    fn wait(&self, id: JobId) -> JobRecord {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let job = self.core.job(id).unwrap();
            if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
            assert!(Instant::now() < until, "job timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn run(&self, op: Operation) -> JobRecord {
        let job = self.core.submit(op).unwrap();
        let done = self.wait(job.id);
        assert_eq!(done.state, JobState::Succeeded, "{done:?}");
        done
    }
    fn finish_plan(&self, reader: &CryptomatorReader, plan: &CryptomatorPlan) {
        loop {
            match reader.advance_plan(&self.core, plan).unwrap() {
                CryptomatorPlanStatus::Completed => return,
                CryptomatorPlanStatus::Submitted { job, .. } | CryptomatorPlanStatus::Running { job, .. } => { self.wait(job); }
                stopped => panic!("plan stopped: {stopped:?} {:?}", self.core.job(plan.job_id(0)).ok().map(|j| j.error)),
            }
        }
    }
}

const PASSWORD: &str = "interop-password";
fn long_name() -> String { "very long file name ".repeat(12).trim().to_owned() + ".txt" }
fn long_dir() -> String { "long directory ".repeat(15).trim().to_owned() }
fn decrypt(reader: &CryptomatorReader, path: &[String]) -> Vec<u8> {
    let (out, data) = sink();
    reader.decrypt_file(path, out, Default::default(), CryptomatorLengthPolicy::AllowUnauthenticated, |_| Ok(())).unwrap();
    assert!(data.borrow().committed);
    let bytes = data.borrow().bytes.clone();
    bytes
}
fn import(h: &Harness, base: &std::path::Path, reader: &CryptomatorReader, parent: &[String], name: &str, data: &[u8]) {
    let (spool, bytes) = sink();
    let prepared = reader.prepare_file_import(parent, name, data, spool, Default::default(), |_| Ok(())).unwrap();
    let spool_name = format!("spool-{}", uuid::Uuid::new_v4().simple());
    std::fs::write(base.join(&spool_name), &bytes.borrow().bytes).unwrap();
    let job = reader.submit_file_import(&h.core, &prepared, h.loc(0, &spool_name), id()).unwrap();
    assert_eq!(h.wait(job.id).state, JobState::Succeeded);
    std::fs::remove_file(base.join(&spool_name)).unwrap();
}

#[test]
fn cryptomator_vaults_from_cryptofs_read_and_write() {
    let _kdf = KDF_INTEROP.lock().unwrap();
    let Ok(base) = std::env::var("PARANOIC_CRYPTOMATOR_FIXTURES") else {
        interop_skip("PARANOIC_CRYPTOMATOR_FIXTURES"); return;
    };
    let h = Harness::new(&[PathBuf::from(&base)]);
    for (vault, combo) in [("v8-gcm", CryptomatorCipherCombo::SivGcm), ("v8-ctrmac", CryptomatorCipherCombo::SivCtrMac)] {
        let reader = CryptomatorReader::open(h.storages[0].clone(), ResourcePath::parse(vault).unwrap(),
            Zeroizing::new(PASSWORD.into()), Duration::from_secs(600), || Ok(())).unwrap();
        // Read everything Cryptomator wrote.
        let mut names: Vec<String> = reader.list_directory(&[], 100, || Ok(())).unwrap().entries().iter().map(|e| e.name().to_owned()).collect();
        names.sort();
        let mut expected = vec![s("dir"), s("empty.txt"), s("hello.txt"), s("link"), s("multi-chunk.bin"), long_dir(), long_name()];
        expected.sort();
        assert_eq!(names, expected, "{vault}");
        assert_eq!(decrypt(&reader, &[s("empty.txt")]), b"");
        assert_eq!(decrypt(&reader, &[s("hello.txt")]), b"hello from cryptomator");
        assert_eq!(decrypt(&reader, &[s("multi-chunk.bin")]), pattern(100_000, 5));
        assert_eq!(decrypt(&reader, &[long_name()]), pattern(1000, 6));
        assert_eq!(decrypt(&reader, &[s("dir"), s("nested"), s("deep.txt")]), b"deep");
        // Random access (E4.7) on files written by Cryptomator itself.
        let mut file = reader.open_file_reader(&[s("multi-chunk.bin")]).unwrap();
        assert_eq!(file.size(), 100_000);
        assert_eq!(&file.read_at(40_000, 30_000).unwrap()[..], &pattern(100_000, 5)[40_000..70_000]);
        assert_eq!(reader.open_file_reader(&[s("empty.txt")]).unwrap().size(), 0);
        assert_eq!(&reader.open_file_reader(&[long_name()]).unwrap().read_at(10, 20).unwrap()[..], &pattern(1000, 6)[10..30]);
        assert_eq!(decrypt(&reader, &[long_dir(), s("inside.txt")]), b"inside long dir");
        assert_eq!(reader.read_symlink(&[s("link")], || Ok(())).unwrap().as_str(), "dir/nested/deep.txt");
        assert_eq!(reader.cipher_combo().unwrap(), combo);
        // Write with Paranoic; tools/interop/cryptomator `verify` checks the result.
        import(&h, std::path::Path::new(&base), &reader, &[], "from-paranoic.bin", &pattern(70_000, 9));
        let prepared = reader.prepare_directory(&[s("dir")], "Paranoic folder", || Ok(())).unwrap();
        let job = reader.submit_directory(&h.core, &prepared, id()).unwrap();
        assert_eq!(h.wait(job.id).state, JobState::Succeeded);
        import(&h, std::path::Path::new(&base), &reader, &[s("dir"), s("Paranoic folder")], &(long_name() + ".paranoic"), &pattern(500, 10));
        let rename = reader.prepare_rename(&[], "hello.txt", "renamed-hello.txt", || Ok(())).unwrap();
        let job = reader.submit_rename(&h.core, &rename, id()).unwrap();
        assert_eq!(h.wait(job.id).state, JobState::Succeeded);
        let plan = reader.prepare_delete(&[], "multi-chunk.bin", Default::default(), uuid::Uuid::new_v4(), || Ok(())).unwrap();
        h.finish_plan(&reader, &plan);
        let plan = reader.prepare_delete(&[], &long_dir(), Default::default(), uuid::Uuid::new_v4(), || Ok(())).unwrap();
        assert!(plan.steps().len() >= 3);
        let plan = CryptomatorPlan::from_json(&plan.to_json().unwrap()).unwrap();
        h.finish_plan(&reader, &plan);
        let names: Vec<String> = reader.list_directory(&[], 100, || Ok(())).unwrap().entries().iter().map(|e| e.name().to_owned()).collect();
        assert!(!names.contains(&long_dir()) && !names.contains(&s("multi-chunk.bin")) && names.contains(&s("renamed-hello.txt")));
    }
}

#[test]
fn cryptomator_format7_vault_from_cryptofs_1_9() {
    let _kdf = KDF_INTEROP.lock().unwrap();
    let Ok(base) = std::env::var("PARANOIC_CRYPTOMATOR_V7_FIXTURES") else {
        interop_skip("PARANOIC_CRYPTOMATOR_V7_FIXTURES"); return;
    };
    let h = Harness::new(&[PathBuf::from(&base)]);
    let reader = CryptomatorReader::open(h.storages[0].clone(), ResourcePath::parse("v7").unwrap(),
        Zeroizing::new(PASSWORD.into()), Duration::from_secs(600), || Ok(())).unwrap();
    assert_eq!(reader.format().unwrap(), 7);
    assert_eq!(reader.cipher_combo().unwrap(), CryptomatorCipherCombo::SivCtrMac);
    assert!(CryptomatorReader::open(h.storages[0].clone(), ResourcePath::parse("v7").unwrap(),
        Zeroizing::new("wrong".into()), Duration::from_secs(60), || Ok(())).is_err());
    let mut names: Vec<String> = reader.list_directory(&[], 100, || Ok(())).unwrap().entries().iter().map(|e| e.name().to_owned()).collect();
    names.sort();
    let mut expected = vec![s("dir"), s("hello.txt"), s("multi-chunk.bin"), long_name()];
    expected.sort();
    assert_eq!(names, expected);
    assert_eq!(decrypt(&reader, &[s("hello.txt")]), b"hello from cryptomator 7");
    assert_eq!(decrypt(&reader, &[s("multi-chunk.bin")]), pattern(100_000, 5));
    assert_eq!(decrypt(&reader, &[long_name()]), pattern(1000, 6));
    assert_eq!(decrypt(&reader, &[s("dir"), s("deep.txt")]), b"deep");
    let mut file = reader.open_file_reader(&[s("multi-chunk.bin")]).unwrap();
    assert_eq!(&file.read_at(32_000, 40_000).unwrap()[..], &pattern(100_000, 5)[32_000..72_000]);
    import(&h, std::path::Path::new(&base), &reader, &[s("dir")], "from-paranoic.bin", &pattern(40_000, 9));
    let rename = reader.prepare_rename(&[], "hello.txt", "renamed.txt", || Ok(())).unwrap();
    let job = reader.submit_rename(&h.core, &rename, id()).unwrap();
    assert_eq!(h.wait(job.id).state, JobState::Succeeded);
}

fn mtime(path: &std::path::Path) -> std::time::SystemTime { std::fs::metadata(path).unwrap().modified().unwrap() }

#[test]
fn core_conflicts_cross_storage_moves_and_times() {
    let (a, b) = (Temp::new(), Temp::new());
    let h = Harness::new(&[a.0.clone(), b.0.clone()]);
    std::fs::write(a.0.join("doc.txt"), b"new").unwrap();
    let old_time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    std::fs::File::options().write(true).open(a.0.join("doc.txt")).unwrap().set_modified(old_time).unwrap();
    std::fs::write(a.0.join("existing.txt"), b"old").unwrap();
    // KeepBoth: "existing (1).txt", then "(2)"; the chosen target is reported.
    for expected in ["existing (1).txt", "existing (2).txt"] {
        let job = h.run(Operation::CopyFile { source: h.loc(0, "doc.txt"), target: h.loc(0, "existing.txt"), conflict: ConflictPolicy::KeepBoth });
        assert_eq!(job.result.unwrap().target.unwrap().path.as_str(), expected);
        assert_eq!(std::fs::read(a.0.join(expected)).unwrap(), b"new");
    }
    assert_eq!(std::fs::read(a.0.join("existing.txt")).unwrap(), b"old");
    // Modification time is preserved on copies.
    assert_eq!(mtime(&a.0.join("existing (1).txt")), old_time);
    // Replace: file over file, no hidden leftovers.
    h.run(Operation::CopyFile { source: h.loc(0, "doc.txt"), target: h.loc(0, "existing.txt"), conflict: ConflictPolicy::Replace });
    assert_eq!(std::fs::read(a.0.join("existing.txt")).unwrap(), b"new");
    assert!(!std::fs::read_dir(&a.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-")));
    // Replace never replaces a directory with a file.
    std::fs::create_dir(a.0.join("folder")).unwrap();
    let job = h.core.submit(Operation::CopyFile { source: h.loc(0, "doc.txt"), target: h.loc(0, "folder"), conflict: ConflictPolicy::Replace }).unwrap();
    assert_eq!(h.wait(job.id).state, JobState::Failed);
    // KeepBoth for a directory copy.
    std::fs::write(a.0.join("folder/inner.bin"), pattern(5000, 1)).unwrap();
    let job = h.run(Operation::CopyDirectory { source: h.loc(0, "folder"), target: h.loc(0, "folder"), conflict: ConflictPolicy::KeepBoth });
    assert_eq!(job.result.unwrap().target.unwrap().path.as_str(), "folder (1)");
    assert_eq!(std::fs::read(a.0.join("folder (1)/inner.bin")).unwrap(), pattern(5000, 1));
    // Duplicate a file in place ("Copy" next to itself).
    let job = h.run(Operation::CopyFile { source: h.loc(0, "doc.txt"), target: h.loc(0, "doc.txt"), conflict: ConflictPolicy::KeepBoth });
    assert_eq!(job.result.unwrap().target.unwrap().path.as_str(), "doc (1).txt");
    // Cross-storage file move: destination verified, source removed.
    h.run(Operation::MoveFileAcross { source: h.loc(0, "doc.txt"), target: h.loc(1, "moved.txt") });
    assert!(!a.0.join("doc.txt").exists());
    assert_eq!(std::fs::read(b.0.join("moved.txt")).unwrap(), b"new");
    // Cross-storage directory move with nested content.
    std::fs::create_dir_all(a.0.join("tree/sub")).unwrap();
    std::fs::write(a.0.join("tree/sub/x.bin"), pattern(200_000, 2)).unwrap();
    std::fs::write(a.0.join("tree/y.txt"), b"y").unwrap();
    h.run(Operation::MoveDirectoryAcross { source: h.loc(0, "tree"), target: h.loc(1, "tree") });
    assert!(!a.0.join("tree").exists());
    assert_eq!(std::fs::read(b.0.join("tree/sub/x.bin")).unwrap(), pattern(200_000, 2));
    assert_eq!(std::fs::read(b.0.join("tree/y.txt")).unwrap(), b"y");
    // Same-storage MoveDirectoryAcross is rejected (use Move).
    assert!(h.core.submit(Operation::MoveDirectoryAcross { source: h.loc(1, "tree"), target: h.loc(1, "tree2") }).is_err());
}

#[test]
fn cryptomator_plans_password_change_and_file_reshape() {
    let _kdf = KDF_INTEROP.lock().unwrap();
    let root = Temp::new();
    let h = Harness::new(&[root.0.clone()]);
    let bootstrap = CryptomatorVault::prepare_create(Zeroizing::new(PASSWORD.into()), || Ok(())).unwrap();
    let job = bootstrap.submit(&h.core, h.loc(0, "vault"), id()).unwrap();
    assert_eq!(h.wait(job.id).state, JobState::Succeeded);
    let open = |password: &str| CryptomatorReader::open(h.storages[0].clone(), ResourcePath::parse("vault").unwrap(),
        Zeroizing::new(password.into()), Duration::from_secs(600), || Ok(()));
    let reader = open(PASSWORD).unwrap();
    // File c9r -> c9s -> c9r without re-encryption.
    import(&h, &root.0, &reader, &[], "short.txt", b"reshaped content");
    let long = "x".repeat(200);
    let plan = reader.prepare_file_reshape(&[], "short.txt", &[], &long, uuid::Uuid::new_v4(), || Ok(())).unwrap();
    h.finish_plan(&reader, &plan);
    assert_eq!(decrypt(&reader, &[long.clone()]), b"reshaped content");
    let plan = reader.prepare_file_reshape(&[], &long, &[], "back.txt", uuid::Uuid::new_v4(), || Ok(())).unwrap();
    h.finish_plan(&reader, &plan);
    assert_eq!(decrypt(&reader, &[s("back.txt")]), b"reshaped content");
    let names: Vec<String> = reader.list_directory(&[], 100, || Ok(())).unwrap().entries().iter().map(|e| e.name().to_owned()).collect();
    assert_eq!(names, vec![s("back.txt")]);
    // Durable password change, then removal of the old-password backup.
    let change = reader.prepare_password_change_plan(Zeroizing::new("new password".into()), uuid::Uuid::new_v4(), || Ok(())).unwrap();
    h.finish_plan(&reader, &change);
    assert!(open(PASSWORD).is_err());
    let reopened = open("new password").unwrap();
    assert_eq!(decrypt(&reopened, &[s("back.txt")]), b"reshaped content");
    let backup = root.0.join(format!("vault/masterkey.cryptomator.{}.bkup", change.plan_id().simple()));
    assert!(backup.exists());
    assert!(reopened.prepare_remove_masterkey_backup(&change, Zeroizing::new("wrong".into()), uuid::Uuid::new_v4(), || Ok(())).is_err());
    let removal = reopened.prepare_remove_masterkey_backup(&change, Zeroizing::new("new password".into()), uuid::Uuid::new_v4(), || Ok(())).unwrap();
    h.finish_plan(&reopened, &removal);
    assert!(!backup.exists());
}

/// Audit 18: with PARANOIC_REQUIRE_INTEROP set, a missing fixture or tool fails
/// the run instead of passing as a silent skip.
#[allow(dead_code)]
fn interop_skip(what: &str) {
    assert!(std::env::var_os("PARANOIC_REQUIRE_INTEROP").is_none(), "{what} required by PARANOIC_REQUIRE_INTEROP");
    eprintln!("{what}: skipped");
}
