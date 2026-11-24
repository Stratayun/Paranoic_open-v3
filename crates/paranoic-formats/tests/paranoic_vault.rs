//! Paranoic format v1 vault through Core: on a local folder, and (when the
//! server binary is available) on our own WebDAV server via CloudStorage.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::Write, path::PathBuf, rc::Rc, sync::Arc, time::Duration};
use paranoic_core::*;
use paranoic_crypto::StreamSink;
use paranoic_formats::paranoic_v1::{Entry, EntryKind, KdfParams, NewFile, NewPlace};
use paranoic_formats::paranoic_vault::*;
use paranoic_local::{Access, LocalStorage};

const FAST: KdfParams = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };
const PASSWORD: &[u8] = b"vault password for tests";
const LIMIT: Duration = Duration::from_secs(60);
fn ok() -> paranoic_crypto::Result<()> { Ok(()) }

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-vault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn dir(&self, name: &str) -> PathBuf { let p = self.0.join(name); std::fs::create_dir_all(&p).unwrap(); p }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

#[derive(Default)] struct Out { data: Vec<u8>, committed: bool, aborted: bool }
struct Sink(Rc<RefCell<Out>>, Vec<u8>);
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.1.extend_from_slice(b); Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { let mut o = self.0.borrow_mut(); o.data = self.1.clone(); o.committed = true; Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().aborted = true; Ok(()) }
}

fn run(core: &Core, plan: &VaultPlan) -> Vec<JobRecord> { run_plan(core, plan, LIMIT).unwrap() }
/// Run a write plan through the version manifest (spec §9); the manifest
/// step's own record is left out so step counts stay those of the entry plan.
fn runm(core: &Core, vault: &ParanoicVault, spool: &dyn Storage, plan: VaultPlan) -> Vec<JobRecord> {
    let path = ResourcePath::parse(format!("m-{}", uuid::Uuid::new_v4().simple())).unwrap();
    let before = plan.steps.len();
    let plan = vault.with_manifest(core, plan, Spool { storage: spool, path: &path }).unwrap();
    let mut records = run_plan(core, &plan, LIMIT).unwrap();
    let succeeded = records.iter().all(|r| r.state == JobState::Succeeded);
    if plan.steps.len() > before && records.first().is_some_and(|r| r.state == JobState::Succeeded) { records.remove(0); }
    if succeeded {
        vault.confirm(&plan).unwrap();
        if let Some(settle) = vault.plan_settle(core, Spool { storage: spool, path: &path }).unwrap() {
            assert!(run_plan(core, &settle, LIMIT).unwrap().iter().all(|r| r.state == JobState::Succeeded), "settle failed");
            vault.confirm(&settle).unwrap();
        }
    }
    let _ = spool.delete_file(&manifest_spool_path(&path).unwrap());
    records
}
fn all_ok(records: &[JobRecord], expected_steps: usize) {
    assert_eq!(records.len(), expected_steps, "{records:?}");
    assert!(records.iter().all(|r| r.state == JobState::Succeeded), "{records:?}");
}
fn failed(records: &[JobRecord]) -> bool { records.last().is_some_and(|r| r.state != JobState::Succeeded) }
fn read(vault: &ParanoicVault, core: &Core, folder: &[u8; 16], name: &str) -> (Result<Entry>, Rc<RefCell<Out>>) {
    let out = Rc::new(RefCell::new(Out::default()));
    let r = vault.read_file(core, folder, name, Box::new(Sink(out.clone(), Vec::new())), u64::MAX, &mut |_| Ok(()));
    (r, out)
}
fn content(vault: &ParanoicVault, core: &Core, folder: &[u8; 16], name: &str) -> Vec<u8> {
    let (r, out) = read(vault, core, folder, name); r.unwrap(); let v = out.borrow().data.clone(); v
}

/// The whole scenario on any vault location (local folder or WebDAV).
/// `raw` gives direct access to the stored objects for attack simulation.
fn scenario(core: &Core, vault_location: Location, spool: &dyn Storage, raw: Option<&std::path::Path>) {
    let mut n = 0u32;
    let mut spool_path = || { n += 1; ResourcePath::parse(format!("spool-{n}")).unwrap() };
    // Create.
    let (plan, _) = ParanoicVault::plan_create(vault_location.clone(), PASSWORD, FAST, true, &mut ok).unwrap();
    let steps = plan.steps.len();
    all_ok(&run(core, &plan), steps);
    assert!(ParanoicVault::open(core, vault_location.clone(), b"wrong password", &mut ok).is_err());
    let vault = ParanoicVault::open(core, vault_location.clone(), PASSWORD, &mut ok).unwrap();
    let root = vault.root_folder();
    // Add a file; a second add of the same name is refused.
    let data: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
    let p = spool_path();
    let prepared = vault.prepare_file(&root, NewFile { name: "Отчёт.txt", modified_ms: 11, size: data.len() as u64 }, data.as_slice(),
        Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    all_ok(&runm(core, &vault, spool, vault.plan_add(&prepared)), 1);
    assert_eq!(content(&vault, core, &root, "Отчёт.txt"), data);
    let p = spool_path();
    let again = vault.prepare_file(&root, NewFile { name: "Отчёт.txt", modified_ms: 12, size: 3 }, b"new".as_slice(),
        Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    assert!(failed(&runm(core, &vault, spool, vault.plan_add(&again))), "existing name overwritten");
    assert_eq!(content(&vault, core, &root, "Отчёт.txt"), data);
    // Replace: stale expectation refused, current one accepted.
    let current = vault.stat(core, &root, "Отчёт.txt", EntryKind::File).unwrap();
    let stale = Metadata { bytes: current.bytes + 1, ..current.clone() };
    assert!(failed(&runm(core, &vault, spool, vault.plan_replace(&again, stale))));
    all_ok(&runm(core, &vault, spool, vault.plan_replace(&again, current)), 1);
    assert_eq!(content(&vault, core, &root, "Отчёт.txt"), b"new");
    // Folders.
    let p = spool_path();
    let (_, plan) = vault.prepare_folder(core, &root, "Папка", 13, Spool { storage: spool, path: &p }).unwrap();
    let steps = plan.steps.len();
    all_ok(&runm(core, &vault, spool, plan), steps);
    let sub = vault.resolve(core, &["Папка".to_string()]).unwrap();
    assert_ne!(sub, root);
    let p = spool_path();
    let inner = vault.prepare_file(&sub, NewFile { name: "inner.bin", modified_ms: 14, size: 4 }, b"abcd".as_slice(),
        Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    all_ok(&runm(core, &vault, spool, vault.plan_add(&inner)), 1);
    let listing = vault.list(core, &root).unwrap();
    assert_eq!(listing.damaged, 0);
    let names: Vec<(&str, bool)> = listing.entries.iter().map(|e| (e.name.as_str(), e.folder)).collect();
    assert_eq!(names, [("Папка", true), ("Отчёт.txt", false)]);
    assert_eq!(listing.entries[1].size, 3);
    // Rename a file and move it into the folder: content kept, old name gone.
    let p = spool_path();
    let plan = vault.prepare_rename(core, EntryRef { folder: &root, name: "Отчёт.txt", kind: EntryKind::File },
        NewPlace { folder: &sub, name: "moved.txt", modified_ms: 15 }, Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    all_ok(&runm(core, &vault, spool, plan), 2);
    assert_eq!(content(&vault, core, &sub, "moved.txt"), b"new");
    assert!(read(&vault, core, &root, "Отчёт.txt").0.is_err());
    // Rename the folder: its contents follow (same folder ID).
    let p = spool_path();
    let plan = vault.prepare_rename(core, EntryRef { folder: &root, name: "Папка", kind: EntryKind::Folder },
        NewPlace { folder: &root, name: "Архив", modified_ms: 16 }, Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    all_ok(&runm(core, &vault, spool, plan), 2);
    assert_eq!(vault.resolve(core, &["Архив".to_string()]).unwrap(), sub);
    assert_eq!(content(&vault, core, &sub, "inner.bin"), b"abcd");
    // A non-empty folder cannot be deleted; files then the empty folder can.
    assert!(vault.plan_delete_folder(core, &root, "Архив").is_err());
    all_ok(&runm(core, &vault, spool, vault.plan_delete_file(core, &sub, "inner.bin").unwrap()), 1);
    all_ok(&runm(core, &vault, spool, vault.plan_delete_file(core, &sub, "moved.txt").unwrap()), 1);
    let plan = vault.plan_delete_folder(core, &root, "Архив").unwrap();
    let steps = plan.steps.len();
    all_ok(&runm(core, &vault, spool, plan), steps);
    assert!(vault.list(core, &root).unwrap().entries.is_empty());

    // Attacks on the stored objects (local storage only: direct access).
    let Some(raw) = raw else { return };
    for (name, body) in [("a.txt", b"first secret".as_slice()), ("b.txt", b"second secret".as_slice())] {
        let p = spool_path();
        let prepared = vault.prepare_file(&root, NewFile { name, modified_ms: 1, size: body.len() as u64 }, body,
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        all_ok(&runm(core, &vault, spool, vault.plan_add(&prepared)), 1);
    }
    let object = |name: &str| -> PathBuf {
        let folder = vault.keys().folder_path(&root).unwrap();
        raw.join(folder).join(vault.keys().entry_name(&root, name, EntryKind::File).unwrap())
    };
    // No plaintext names or contents anywhere on disk.
    for entry in walk(raw) {
        let bytes = std::fs::read(&entry).unwrap();
        for needle in [b"secret".as_slice(), "Отчёт".as_bytes(), b"a.txt", "Архив".as_bytes()] {
            assert!(!bytes.windows(needle.len()).any(|w| w == needle), "plaintext in {entry:?}");
            assert!(!entry.to_string_lossy().as_bytes().windows(needle.len()).any(|w| w == needle), "plaintext name {entry:?}");
        }
    }
    // Swap: b's object copied over a's name is rejected, nothing released.
    let (a, b) = (object("a.txt"), object("b.txt"));
    let original_a = std::fs::read(&a).unwrap();
    std::fs::copy(&b, &a).unwrap();
    let (r, out) = read(&vault, core, &root, "a.txt");
    assert!(r.is_err(), "swapped object accepted");
    assert!(!out.borrow().committed && out.borrow().data.is_empty());
    assert_eq!(vault.list(core, &root).unwrap().damaged, 1, "swapped object must be reported, not listed");
    // Bit flip in content.
    let mut flipped = original_a.clone(); let last = flipped.len() - 1; flipped[last] ^= 1;
    std::fs::write(&a, &flipped).unwrap();
    assert!(read(&vault, core, &root, "a.txt").0.is_err());
    std::fs::write(&a, &original_a).unwrap();
    assert_eq!(content(&vault, core, &root, "a.txt"), b"first secret");

    // Master-key rotation into a new vault: same tree, new keys, old vault untouched.
    let p = spool_path();
    let (_, plan) = vault.prepare_folder(core, &root, "Sub", 2, Spool { storage: spool, path: &p }).unwrap();
    let steps = plan.steps.len(); all_ok(&runm(core, &vault, spool, plan), steps);
    let sub = vault.resolve(core, &["Sub".to_string()]).unwrap();
    let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let p = spool_path();
    let prepared = vault.prepare_file(&sub, NewFile { name: "big.bin", modified_ms: 3, size: big.len() as u64 }, big.as_slice(),
        Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    all_ok(&runm(core, &vault, spool, vault.plan_add(&prepared)), 1);
    let before: Vec<(PathBuf, Vec<u8>)> = walk(raw).into_iter().map(|p| { let b = std::fs::read(&p).unwrap(); (p, b) }).collect();
    let target = Location { storage: vault_location.storage, path: ResourcePath::parse(format!("{}-rotated", vault_location.path.as_str())).unwrap() };
    let rotated = vault.rotate_into(core, target.clone(), VaultSettings { password: b"rotated password!", kdf: FAST, padding: false },
        spool, LIMIT, &mut ok).unwrap();
    assert_ne!(rotated.root_folder(), vault.root_folder(), "new vault identity");
    let reopened = ParanoicVault::open(core, target.clone(), b"rotated password!", &mut ok).unwrap();
    assert!(ParanoicVault::open(core, target, PASSWORD, &mut ok).is_err(), "old password opens the rotated vault");
    let new_root = reopened.root_folder();
    let names: Vec<String> = reopened.list(core, &new_root).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Sub", "a.txt", "b.txt"]);
    assert_eq!(content(&reopened, core, &new_root, "a.txt"), b"first secret");
    let new_sub = reopened.resolve(core, &["Sub".to_string()]).unwrap();
    assert_eq!(content(&reopened, core, &new_sub, "big.bin"), big);
    // The old vault is byte-for-byte unchanged.
    for (path, bytes) in before { assert_eq!(std::fs::read(&path).unwrap(), bytes, "old vault modified: {path:?}"); }
    // A damaged old vault is not rotated (stops before copying anything wrong).
    std::fs::copy(object("b.txt"), object("a.txt")).unwrap();
    let target = Location { storage: vault_location.storage, path: ResourcePath::parse(format!("{}-r2", vault_location.path.as_str())).unwrap() };
    assert!(vault.rotate_into(core, target, VaultSettings { password: b"x password long", kdf: FAST, padding: true }, spool, LIMIT, &mut ok).is_err());
    std::fs::write(&a, &original_a).unwrap();
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { out.extend(walk(&path)); } else { out.push(path); }
    }
    out
}

#[test] fn local_folder_vault_through_core() {
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool.clone()).unwrap();
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    scenario(&core, location, spool.as_ref(), Some(&temp.0.join("files/Vault")));
}

#[test] fn vault_on_own_webdav_server() {
    let server = std::env::var("PARANOIC_SERVER").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-server"));
    if !server.exists() { eprintln!("paranoic-server not built: WebDAV vault test skipped"); return; }
    let temp = Temp::new();
    let root = temp.dir("served");
    let token = "test-token-0123456789abcdefghijklmnop";
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = temp.0.join("config.json");
    std::fs::write(&config, serde_json::json!({ "root": root, "listen": format!("127.0.0.1:{port}"), "username": "me",
        "token_blake3": blake3::hash(token.as_bytes()).to_hex().to_string() }).to_string()).unwrap();
    struct Kill(std::process::Child);
    impl Drop for Kill { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let _server = Kill(std::process::Command::new(&server).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn().unwrap());
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() { assert!(std::time::Instant::now() < until, "server did not start"); std::thread::sleep(Duration::from_millis(50)); }
    let http = paranoic_cloud::Http::new_loopback_http().unwrap();
    let dav = paranoic_cloud::WebDav::new(http, &format!("http://127.0.0.1:{port}/"),
        paranoic_cloud::WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new(token.into()) }, true).unwrap();
    let cloud_spool = temp.dir("cloud-spool");
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&cloud_spool, std::fs::Permissions::from_mode(0o700)).unwrap(); }
    let storage = Arc::new(paranoic_cloud::CloudStorage::open(StorageId(uuid::Uuid::new_v4()), Arc::new(dav),
        paranoic_cloud::CloudId::parse("/").unwrap(), &cloud_spool).unwrap());
    let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(storage.clone()).unwrap(); registry.register(spool.clone()).unwrap();
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let location = Location { storage: storage.id(), path: ResourcePath::parse("Vault").unwrap() };
    // The served folder is the server's disk: attacks are simulated there too.
    scenario(&core, location, spool.as_ref(), Some(&root.join("Vault")));
}

/// Version manifest (spec §9, Р-2): attacks a plain v1 vault would accept.
#[test] fn manifest_detects_rollback_resurrection_and_removal() {
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
    let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let spool: &dyn Storage = spool_storage.as_ref();
    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    let raw = temp.0.join("files/Vault");
    let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, false, &mut ok).unwrap();
    let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
    let vault = ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).unwrap();
    let root = vault.root_folder();
    let mut n = 0u32;
    let mut put = |name: &str, body: &[u8], replace: bool| {
        n += 1; let p = ResourcePath::parse(format!("s-{n}")).unwrap();
        let prepared = vault.prepare_file(&root, NewFile { name, modified_ms: 1, size: body.len() as u64 }, body,
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        let plan = if replace { vault.plan_replace(&prepared, vault.stat(core, &root, name, EntryKind::File).unwrap()) } else { vault.plan_add(&prepared) };
        all_ok(&runm(core, &vault, spool, plan), 1);
    };
    let object = raw.join(vault.keys().folder_path(&root).unwrap()).join(vault.keys().entry_name(&root, "doc.txt", EntryKind::File).unwrap());

    // 1. Single-file rollback: an older authentic version put back is refused.
    put("doc.txt", b"version one", false);
    let old = std::fs::read(&object).unwrap();
    put("doc.txt", b"version two!", true);
    assert_eq!(content(&vault, core, &root, "doc.txt"), b"version two!");
    let new = std::fs::read(&object).unwrap();
    std::fs::write(&object, &old).unwrap();
    let (r, out) = read(&vault, core, &root, "doc.txt");
    assert!(r.is_err(), "rolled-back file accepted");
    assert!(!out.borrow().committed, "rolled-back plaintext released");
    assert_eq!(vault.list(core, &root).unwrap().damaged, 1, "rolled-back file listed");
    std::fs::write(&object, &new).unwrap();
    assert_eq!(content(&vault, core, &root, "doc.txt"), b"version two!");

    // 2. Resurrection: a deleted entry put back is not listed or read.
    all_ok(&runm(core, &vault, spool, vault.plan_delete_file(core, &root, "doc.txt").unwrap()), 1);
    std::fs::write(&object, &new).unwrap();
    assert!(read(&vault, core, &root, "doc.txt").0.is_err(), "deleted file resurrected");
    let listing = vault.list(core, &root).unwrap();
    assert!(listing.entries.is_empty() && listing.damaged == 1, "{listing:?}");
    std::fs::remove_file(&object).unwrap();

    // 3. Interrupted write: only the manifest step ran. The old version stays
    // readable; the next change settles the record against the storage.
    put("doc.txt", b"stable", false);
    let p = ResourcePath::parse("s-interrupted").unwrap();
    let prepared = vault.prepare_file(&root, NewFile { name: "doc.txt", modified_ms: 2, size: 7 }, b"never!!".as_slice(),
        Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
    let plan = vault.with_manifest(core, vault.plan_replace(&prepared, vault.stat(core, &root, "doc.txt", EntryKind::File).unwrap()),
        Spool { storage: spool, path: &p }).unwrap();
    let only_manifest = VaultPlanProbe::first_step(&plan);
    let job = core.submit(only_manifest).unwrap();
    while matches!(core.job(job.id).unwrap().state, JobState::Queued | JobState::Running) { std::thread::sleep(Duration::from_millis(5)); }
    assert_eq!(core.job(job.id).unwrap().state, JobState::Succeeded);
    assert_eq!(content(&vault, core, &root, "doc.txt"), b"stable", "interrupted change broke the old version");
    put("doc.txt", b"after", true);
    assert_eq!(content(&vault, core, &root, "doc.txt"), b"after");

    // 4. Removing the manifest does not fall back to unprotected reading.
    std::fs::remove_file(raw.join("paranoic.manifest")).unwrap();
    assert!(vault.list(core, &root).is_err(), "vault without its manifest listed");
    assert!(read(&vault, core, &root, "doc.txt").0.is_err(), "vault without its manifest read");
}

/// Test-only access to a prepared plan's first step (the manifest replacement).
struct VaultPlanProbe;
impl VaultPlanProbe { fn first_step(plan: &VaultPlan) -> Operation { plan.steps[0].clone() } }

#[derive(Default)]
struct MemoryAnchor(std::sync::Mutex<std::collections::HashMap<[u8; 16], u64>>);
impl ManifestAnchor for MemoryAnchor {
    fn get(&self, id: &[u8; 16]) -> Result<Option<u64>> { Ok(self.0.lock().unwrap().get(id).copied()) }
    fn raise(&self, id: &[u8; 16], sequence: u64) -> Result<()> {
        let mut map = self.0.lock().unwrap();
        let entry = map.entry(*id).or_insert(0); if sequence > *entry { *entry = sequence; } Ok(())
    }
}
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap(); let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() { copy_tree(&entry.path(), &target); } else { std::fs::copy(entry.path(), &target).unwrap(); }
    }
}

/// Spec §9: the manifest is mandatory (a vault without it is refused, not
/// read unprotected); a device anchor detects a whole-vault rollback.
#[test] fn vault_without_manifest_is_refused_and_anchor_detects_whole_vault_rollback() {
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
    let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let spool: &dyn Storage = spool_storage.as_ref();

    // A header without bit 1 (as test builds before the manifest wrote it).
    let (legacy, _) = paranoic_formats::paranoic_v1::create_vault(PASSWORD, FAST, false,
        &mut paranoic_formats::paranoic_v1::OsRandom, &mut ok).unwrap();
    std::fs::create_dir_all(temp.0.join("files/Old")).unwrap();
    std::fs::write(temp.0.join("files/Old/paranoic.vault"), legacy).unwrap();
    let old = Location { storage: files.id(), path: ResourcePath::parse("Old").unwrap() };
    let refused = ParanoicVault::open(core, old, PASSWORD, &mut ok).err().expect("vault without manifest opened");
    assert_eq!(refused.code, ErrorCode::Unsupported);

    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    let raw = temp.0.join("files/Vault");
    let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, false, &mut ok).unwrap();
    let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
    let add = |vault: &ParanoicVault, name: &str, body: &[u8]| -> VaultPlan {
        let p = ResourcePath::parse(format!("s-{}", uuid::Uuid::new_v4().simple())).unwrap();
        let prepared = vault.prepare_file(&vault.root_folder(), NewFile { name, modified_ms: 1, size: body.len() as u64 }, body,
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        vault.plan_add(&prepared)
    };
    let anchor = Arc::new(MemoryAnchor::default());
    let vault = ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).unwrap().with_anchor(anchor.clone());
    let root = vault.root_folder();
    all_ok(&runm(core, &vault, spool, add(&vault, "one.txt", b"first")), 1);
    assert!(vault.list(core, &root).is_ok());
    let backup = temp.0.join("vault-backup");
    copy_tree(&raw, &backup);
    all_ok(&runm(core, &vault, spool, add(&vault, "two.txt", b"second")), 1);
    assert!(vault.list(core, &root).is_ok());
    std::fs::remove_dir_all(&raw).unwrap();
    std::fs::rename(&backup, &raw).unwrap();
    assert!(vault.list(core, &root).is_err(), "whole-vault rollback accepted on a device that saw the newer state");
    // A device that never saw the newer state cannot tell (documented limit).
    let other = ParanoicVault::open(core, location, PASSWORD, &mut ok).unwrap().with_anchor(Arc::new(MemoryAnchor::default()));
    assert!(other.list(core, &root).is_ok());
}

/// Audit of format 1.8 (2026-11-14): finding 1 — renaming must refuse an
/// older authentic version of the source, as reading does; finding 2 — the
/// device anchor pins the published manifest at once after a write.
#[test] fn rename_refuses_stale_source_and_anchor_is_pinned_after_write() {
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
    let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let spool: &dyn Storage = spool_storage.as_ref();
    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    let raw = temp.0.join("files/Vault");
    let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, false, &mut ok).unwrap();
    let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
    let anchor = Arc::new(MemoryAnchor::default());
    let vault = ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).unwrap().with_anchor(anchor.clone());
    let root = vault.root_folder();
    let mut n = 0u32;
    let mut put = |name: &str, body: &[u8], replace: bool| {
        n += 1; let p = ResourcePath::parse(format!("s-{n}")).unwrap();
        let prepared = vault.prepare_file(&root, NewFile { name, modified_ms: 1, size: body.len() as u64 }, body,
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        let plan = if replace { vault.plan_replace(&prepared, vault.stat(core, &root, name, EntryKind::File).unwrap()) } else { vault.plan_add(&prepared) };
        all_ok(&runm(core, &vault, spool, plan), 1);
    };
    let object = raw.join(vault.keys().folder_path(&root).unwrap()).join(vault.keys().entry_name(&root, "doc.txt", EntryKind::File).unwrap());

    // Finding 2: right after a write the anchor already holds the newest manifest,
    // the same value a fresh reader sees.
    put("doc.txt", b"version one", false);
    let old = std::fs::read(&object).unwrap();
    put("doc.txt", b"version two!", true);
    let pinned = anchor.get(&vault.keys().vault_id).unwrap();
    let fresh = Arc::new(MemoryAnchor::default());
    let reader = ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).unwrap().with_anchor(fresh.clone());
    assert!(reader.list(core, &root).is_ok());
    assert_eq!(pinned, fresh.get(&vault.keys().vault_id).unwrap(), "anchor lags behind the published manifest");

    // Finding 1: an older authentic version put back is not renamed into a current entry.
    std::fs::write(&object, &old).unwrap();
    let p = ResourcePath::parse("s-rename").unwrap();
    let renamed = vault.prepare_rename(core, EntryRef { folder: &root, name: "doc.txt", kind: EntryKind::File },
        NewPlace { folder: &root, name: "renamed.txt", modified_ms: 2 }, Spool { storage: spool, path: &p }, &mut |_| Ok(()));
    assert!(renamed.is_err(), "rename accepted a rolled-back source");
    assert!(vault.list(core, &root).unwrap().entries.iter().all(|e| e.name != "renamed.txt"));
}

/// Audit of format 1.8 (2026-11-14), finding 3: an object that vanished from
/// the storage while the manifest records it stops a rotation before anything
/// is created, instead of producing a copy silently missing the file.
#[test] fn rotation_refuses_a_vault_with_a_missing_object() {
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
    let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let spool: &dyn Storage = spool_storage.as_ref();
    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    let raw = temp.0.join("files/Vault");
    let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, false, &mut ok).unwrap();
    let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
    let vault = ParanoicVault::open(core, location, PASSWORD, &mut ok).unwrap();
    let root = vault.root_folder();
    for (n, name) in ["keep.txt", "lost.txt"].iter().enumerate() {
        let p = ResourcePath::parse(format!("s-{n}")).unwrap();
        let prepared = vault.prepare_file(&root, NewFile { name, modified_ms: 1, size: 4 }, &b"data"[..],
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        all_ok(&runm(core, &vault, spool, vault.plan_add(&prepared)), 1);
    }
    assert!(vault.verify_complete(core, &mut ok).is_ok());
    let lost = raw.join(vault.keys().folder_path(&root).unwrap()).join(vault.keys().entry_name(&root, "lost.txt", EntryKind::File).unwrap());
    std::fs::remove_file(&lost).unwrap();
    // A plain listing cannot tell: it shows what is there, nothing damaged.
    let listing = vault.list(core, &root).unwrap();
    assert_eq!((listing.entries.len(), listing.damaged), (1, 0));
    let target = Location { storage: files.id(), path: ResourcePath::parse("Rotated").unwrap() };
    let refused = vault.rotate_into(core, target, VaultSettings { password: b"rotated password!", kdf: FAST, padding: false }, spool, LIMIT, &mut ok);
    assert_eq!(refused.err().expect("rotation accepted a vault with a missing file").code, ErrorCode::Integrity);
    assert!(!temp.0.join("files/Rotated").exists(), "a partial copy was created");
}

/// E5.4 / R-8: forgotten password — the recovery key opens the vault and sets
/// a new password; the old password stops working, the key keeps working.
/// The recovery key window closes only after the user re-types random groups.
#[test] fn recovery_key_resets_the_password_and_challenge_checks_groups() {
    use paranoic_formats::paranoic_v1::{self as v1, RandomSource};
    let temp = Temp::new();
    let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
    let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
    let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let spool: &dyn Storage = spool_storage.as_ref();
    let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
    let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, true, &mut ok).unwrap();
    let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
    let vault = ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).unwrap();
    let path = |n: &str| ResourcePath::parse(n).unwrap();
    let (plan, text) = vault.plan_set_recovery(core, PASSWORD, Spool { storage: spool, path: &path("r1") }, &mut ok).unwrap();
    all_ok(&run(core, &plan), 1);

    // Challenge: 2–3 distinct ascending groups; answers in any case, with spaces.
    let groups: Vec<&str> = text.split('-').collect();
    assert_eq!(groups.len(), v1::RECOVERY_GROUPS);
    for count in [2, 3] {
        let picked = v1::recovery_challenge(count, &mut v1::OsRandom).unwrap();
        assert_eq!(picked.len(), count);
        assert!(picked.windows(2).all(|w| w[0] < w[1]) && picked.iter().all(|&i| i < v1::RECOVERY_GROUPS));
        let right: Vec<String> = picked.iter().map(|&i| format!(" {} ", groups[i].to_ascii_uppercase())).collect();
        let right: Vec<&str> = right.iter().map(String::as_str).collect();
        assert!(v1::recovery_check(&text, &picked, &right));
        let mut wrong = right.clone(); wrong[0] = "zzzz";
        assert!(!v1::recovery_check(&text, &picked, &wrong));
        assert!(!v1::recovery_check(&text, &picked, &right[..1]), "fewer answers than groups");
    }
    assert!(v1::recovery_challenge(1, &mut v1::OsRandom).is_err() && v1::recovery_challenge(4, &mut v1::OsRandom).is_err());
    // Rejection sampling: bytes ≥ 247 are skipped, repeats are skipped.
    struct Fixed(Vec<u8>);
    impl RandomSource for Fixed { fn fill(&mut self, out: &mut [u8]) -> paranoic_crypto::Result<()> { out[0] = self.0.remove(0); Ok(()) } }
    assert_eq!(v1::recovery_challenge(2, &mut Fixed(vec![250, 14, 1, 28])).unwrap(), [1, 2]);

    // Forgotten password: open with the key, set a new one.
    let key = v1::parse_recovery_text(&text).unwrap();
    let restored = ParanoicVault::open_recovery(core, location.clone(), &key).unwrap();
    let wrong_key = [7u8; 32];
    assert!(restored.plan_reset_password(core, &wrong_key, b"new password long enough", Spool { storage: spool, path: &path("r2") }, &mut ok).is_err());
    let plan = restored.plan_reset_password(core, &key, b"new password long enough", Spool { storage: spool, path: &path("r3") }, &mut ok).unwrap();
    all_ok(&run(core, &plan), 1);
    assert!(ParanoicVault::open(core, location.clone(), PASSWORD, &mut ok).is_err(), "old password still opens");
    ParanoicVault::open(core, location.clone(), b"new password long enough", &mut ok).unwrap();
    ParanoicVault::open_recovery(core, location, &key).unwrap();
}

/// Plan 4.7: random-access reading in memory equals the file at any offset;
/// a damaged chunk fails only where it is read; a cut object is refused.
#[test] fn reader_gives_random_access_and_refuses_damage() {
    for padding in [false, true] {
        let temp = Temp::new();
        let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("files"), Access::ReadWrite).unwrap());
        let spool_storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
        let mut registry = StorageRegistry::new();
        registry.register(files.clone()).unwrap(); registry.register(spool_storage.clone()).unwrap();
        let core = &Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
        let spool: &dyn Storage = spool_storage.as_ref();
        let location = Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() };
        let (plan, _) = ParanoicVault::plan_create(location.clone(), PASSWORD, FAST, padding, &mut ok).unwrap();
        let steps = plan.steps.len(); all_ok(&run(core, &plan), steps);
        let vault = ParanoicVault::open(core, location, PASSWORD, &mut ok).unwrap();
        let root = vault.root_folder();
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let p = ResourcePath::parse("s1").unwrap();
        let prepared = vault.prepare_file(&root, NewFile { name: "big.bin", modified_ms: 1, size: body.len() as u64 }, &body[..],
            Spool { storage: spool, path: &p }, &mut |_| Ok(())).unwrap();
        all_ok(&runm(core, &vault, spool, vault.plan_add(&prepared)), 1);

        let mut reader = vault.open_reader(core, &root, "big.bin").unwrap();
        assert_eq!(reader.size(), body.len() as u64);
        for (offset, len) in [(0usize, 10usize), (65_530, 20), (65_536, 65_536), (131_000, 100_000), (199_990, 50), (300_000, 5)] {
            let end = (offset + len).min(body.len());
            let expected = if offset >= body.len() { &[][..] } else { &body[offset..end] };
            assert_eq!(&reader.read_at(offset as u64, len).unwrap()[..], expected, "{offset}+{len} padding={padding}");
        }
        drop(reader);
        // Damage one byte in the second chunk: the first still reads, the second fails.
        let raw = temp.0.join("files/Vault").join(vault.keys().folder_path(&root).unwrap())
            .join(vault.keys().entry_name(&root, "big.bin", EntryKind::File).unwrap());
        let original = std::fs::read(&raw).unwrap();
        let mut damaged = original.clone();
        let at = paranoic_formats::paranoic_v1::ENTRY_HEADER_LEN + 65_536 + 16 + 100;
        damaged[at] ^= 1;
        std::fs::write(&raw, &damaged).unwrap();
        let mut reader = vault.open_reader(core, &root, "big.bin").unwrap();
        assert_eq!(reader.read_at(0, 100).unwrap().len(), 100);
        assert!(reader.read_at(70_000, 10).is_err(), "a damaged chunk was accepted");
        drop(reader);
        std::fs::write(&raw, &original[..original.len() - 1]).unwrap();
        assert!(vault.open_reader(core, &root, "big.bin").is_err(), "a cut file was opened");
    }
}
