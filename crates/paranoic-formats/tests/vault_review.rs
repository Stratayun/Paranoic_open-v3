//! Regression cases from the 1.8 review. Only disposable local data.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::Write, rc::Rc, sync::Arc, time::{Duration, UNIX_EPOCH}};
use paranoic_core::*;
use paranoic_crypto::{CryptoError, StreamSink};
use paranoic_formats::{paranoic_v1::{EntryKind, KdfParams, NewFile, padme}, paranoic_vault::*};
use paranoic_local::{Access, LocalStorage};

struct Fixture { root: std::path::PathBuf, core: Core, files: Arc<LocalStorage>, vault: ParanoicVault }
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("paranoic-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &root, Access::ReadWrite).unwrap());
        let mut registry = StorageRegistry::new(); registry.register(files.clone()).unwrap();
        let core = Core::open(root.join("state"), CoreConfig::default(), registry).unwrap();
        let (plan, vault) = ParanoicVault::plan_create(Location { storage: files.id(), path: ResourcePath::parse("Vault").unwrap() },
            b"review password", KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 }, true, &mut || Ok(())).unwrap();
        let fx = Self { root, core, files, vault }; fx.run(plan); fx
    }
    /// Every write passes the version manifest (spec §9); creation has no manifest step.
    fn run(&self, plan: VaultPlan) {
        let path = ResourcePath::parse(format!("m-{}", uuid::Uuid::new_v4().simple())).unwrap();
        let plan = self.vault.with_manifest(&self.core, plan, Spool { storage: self.files.as_ref(), path: &path }).unwrap();
        assert!(run_plan(&self.core, &plan, Duration::from_secs(30)).unwrap().iter().all(|r| r.state == JobState::Succeeded));
        if let Some(settle) = self.vault.plan_settle(&self.core, Spool { storage: self.files.as_ref(), path: &path }).unwrap() {
            assert!(run_plan(&self.core, &settle, Duration::from_secs(30)).unwrap().iter().all(|r| r.state == JobState::Succeeded));
        }
    }
    fn add(&self, folder: &[u8; 16], name: &str) {
        let path = ResourcePath::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        let prepared = self.vault.prepare_file(folder, NewFile { name, size: 6, modified_ms: 1 }, b"secret".as_slice(),
            Spool { storage: self.files.as_ref(), path: &path }, &mut |_| Ok(())).unwrap();
        self.run(self.vault.plan_add(&prepared));
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

#[derive(Default)] struct State { committed: bool, aborted: bool }
struct Sink(Rc<RefCell<State>>);
impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { Ok(data.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(self: Box<Self>) -> std::result::Result<(), CryptoError> { self.0.borrow_mut().committed = true; Ok(()) }
    fn abort(self: Box<Self>) -> std::result::Result<(), CryptoError> { self.0.borrow_mut().aborted = true; Ok(()) }
}

#[test] fn changed_source_never_commits_plaintext() {
    let fx = Fixture::new(); let folder = fx.vault.root_folder(); fx.add(&folder, "note.txt");
    let object = fx.root.join("Vault").join(fx.vault.keys().folder_path(&folder).unwrap())
        .join(fx.vault.keys().entry_name(&folder, "note.txt", EntryKind::File).unwrap());
    let out = Rc::new(RefCell::new(State::default()));
    let result = fx.vault.read_file(&fx.core, &folder, "note.txt", Box::new(Sink(out.clone())), 1024, &mut |_| {
        std::fs::File::open(&object).unwrap().set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(123))).unwrap();
        Ok(())
    });
    assert!(result.is_err());
    assert!(!out.borrow().committed, "plaintext committed despite source-change failure");
    assert!(out.borrow().aborted);
}

#[test] fn folder_filled_after_delete_plan_remains_reachable() {
    let fx = Fixture::new(); let root = fx.vault.root_folder();
    let path = ResourcePath::parse("folder-spool").unwrap();
    let (_, plan) = fx.vault.prepare_folder(&fx.core, &root, "Sub", 1, Spool { storage: fx.files.as_ref(), path: &path }).unwrap();
    fx.run(plan);
    let sub = fx.vault.resolve(&fx.core, &["Sub".into()]).unwrap();
    let delete_spool = ResourcePath::parse("delete-spool").unwrap();
    let delete = fx.vault.with_manifest(&fx.core, fx.vault.plan_delete_folder(&fx.core, &root, "Sub").unwrap(),
        Spool { storage: fx.files.as_ref(), path: &delete_spool }).unwrap();
    fx.add(&sub, "arrived.txt");
    let records = run_plan(&fx.core, &delete, Duration::from_secs(30)).unwrap();
    assert!(records.iter().any(|r| r.state != JobState::Succeeded));
    assert_eq!(fx.vault.resolve(&fx.core, &["Sub".into()]).unwrap(), sub, "non-empty folder orphaned");
    assert_eq!(fx.vault.list(&fx.core, &sub).unwrap().entries[0].name, "arrived.txt");
}

#[test] fn padding_never_wraps_at_u64_boundary() {
    for length in [u64::MAX, u64::MAX - 1, u64::MAX - (1 << 56), 1 << 63] {
        assert!(padme(length) >= length);
    }
}

#[test] fn moving_a_folder_into_its_descendants_is_refused() {
    let fx = Fixture::new(); let root = fx.vault.root_folder();
    let create = |parent: &[u8; 16], name: &str| {
        let path = ResourcePath::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        let (_, plan) = fx.vault.prepare_folder(&fx.core, parent, name, 1, Spool { storage: fx.files.as_ref(), path: &path }).unwrap();
        fx.run(plan);
    };
    create(&root, "Sub"); let sub = fx.vault.resolve(&fx.core, &["Sub".into()]).unwrap();
    create(&sub, "Inner"); let inner = fx.vault.resolve(&fx.core, &["Sub".into(), "Inner".into()]).unwrap();
    for destination in [sub, inner] {
        let path = ResourcePath::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        let result = fx.vault.prepare_rename(&fx.core, EntryRef { folder: &root, name: "Sub", kind: EntryKind::Folder },
            paranoic_formats::paranoic_v1::NewPlace { folder: &destination, name: "cycle", modified_ms: 2 },
            Spool { storage: fx.files.as_ref(), path: &path }, &mut |_| Ok(()));
        assert!(result.is_err(), "move would orphan the whole subtree");
    }
    assert_eq!(fx.vault.resolve(&fx.core, &["Sub".into()]).unwrap(), sub);
}
