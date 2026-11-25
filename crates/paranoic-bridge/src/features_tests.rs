//! Bridge contracts for notes (1.9), encrypted folders (1.8) and WebDAV,
//! exercised through NativeRuntime exactly as the app calls it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf, sync::Mutex, time::Duration};

struct NoDocs;
impl AndroidDocuments for NoDocs {
    fn metadata(&self, _: String, _: String) -> HostInfo { info() }
    fn list(&self, _: String, _: String, _: u32) -> HostListing { HostListing { error: 2, entries: vec![] } }
    fn open(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
    fn read(&self, _: u64, _: u64, _: u32) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn file_metadata(&self, _: u64) -> HostInfo { info() }
    fn close(&self, _: u64) {}
    fn writable(&self, _: String) -> bool { false }
    fn create(&self, _: String, _: String, _: String, _: bool) -> HostCreated { created() }
    fn open_write(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
    fn write(&self, _: u64, _: Vec<u8>) -> u32 { 2 }
    fn finish_write(&self, _: u64) -> u32 { 2 }
    fn rename(&self, _: String, _: String, _: String) -> HostCreated { created() }
    fn delete(&self, _: String, _: String) -> u32 { 2 }
    fn resolve(&self, _: String, _: String) -> HostCreated { created() }
    fn move_document(&self, _: String, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
}
fn info() -> HostInfo { HostInfo { error: 2, kind: 0, bytes: 0, modified_ms: None } }
fn created() -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }

/// Test record key: XOR with a marker (stands in for Keystore / DPAPI).
struct XorSealer;
impl RecordSealer for XorSealer {
    fn available(&self) -> bool { true }
    fn seal(&self, data: Vec<u8>) -> HostBytes { HostBytes { error: 0, data: [b"S1".to_vec(), data.iter().map(|b| b ^ 0x6b).collect()].concat() } }
    fn open(&self, data: Vec<u8>) -> HostBytes {
        match data.strip_prefix(b"S1") { Some(body) => HostBytes { error: 0, data: body.iter().map(|b| b ^ 0x6b).collect() }, None => HostBytes { error: 3, data: vec![] } }
    }
}
struct TestKey(Mutex<bool>);
impl NotesKeyHost for TestKey {
    fn seal(&self, data: Vec<u8>) -> HostBytes { if *self.0.lock().unwrap() { XorSealer.seal(data) } else { HostBytes { error: 1, data: vec![] } } }
    fn open(&self, data: Vec<u8>) -> HostBytes { if *self.0.lock().unwrap() { XorSealer.open(data) } else { HostBytes { error: 1, data: vec![] } } }
    fn erase_key(&self) -> u32 { *self.0.lock().unwrap() = false; 0 }
    fn guarantee(&self) -> u8 { 2 }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("paranoic-features-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap(); }
        Self(dir)
    }
    fn runtime(&self, grants: Vec<NativeGrant>) -> Arc<NativeRuntime> {
        NativeRuntime::new(self.0.join("state").to_str().unwrap().into(), self.0.join("workspace").to_str().unwrap().into(),
            String::new(), String::new(), grants, Arc::new(NoDocs), Arc::new(XorSealer)).unwrap()
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn book(body: &str) -> NativeNotebook {
    NativeNotebook { tabs: vec![NativeTab { id: "t1".into(), title: "Ключи".into(),
        notes: vec![NativeNote { id: "n1".into(), title: "seed".into(), body: body.into(), modified_ms: 5, attachments: Vec::new() }] }] }
}

#[test] fn notes_through_the_bridge() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let key = Arc::new(TestKey(Mutex::new(true)));
    rt.notes_configure(Some(key.clone())).unwrap();
    let status = rt.notes_status().unwrap();
    assert!(!status.set_up && !status.unlocked && !status.auto_wipe);
    assert_eq!(status.guarantee, 2);
    assert!(matches!(rt.notes_configure(None), Err(BridgeError::Busy)), "key changed after the store opened");
    assert_eq!(rt.notes_password_strength("short".into()), 0);
    assert!(matches!(rt.notes_create("short".into()), Err(BridgeError::WeakPassword)));
    let password = "Correct horse battery staple 42";
    rt.notes_create(password.into()).unwrap();
    rt.notes_save(book("abandon ability able about")).unwrap();
    assert_eq!(rt.notes_read().unwrap().tabs[0].notes[0].body, "abandon ability able about");
    rt.notes_lock().unwrap();
    assert!(matches!(rt.notes_read(), Err(BridgeError::Locked)));
    assert!(matches!(rt.notes_unlock("wrong password here".into()), Err(BridgeError::WrongPassword)));
    assert!(matches!(rt.notes_unlock(password.into()), Err(BridgeError::RateLimited { .. })));
    std::thread::sleep(Duration::from_millis(rt.notes_status().unwrap().retry_after_ms + 20));
    rt.notes_unlock(password.into()).unwrap();
    rt.notes_set_auto_wipe(true).unwrap();
    assert!(rt.notes_status().unwrap().auto_wipe);
    // Nothing readable on disk.
    for entry in fs::read_dir(fx.0.join("state/notes")).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        assert!(!bytes.windows(7).any(|w| w == b"abandon"));
    }
    // Shutdown drops the session.
    rt.shutdown().unwrap();
    assert!(matches!(rt.notes_read(), Err(BridgeError::Locked)));
    // Clear destroys the device key and the content.
    assert_eq!(rt.notes_clear().unwrap(), 2);
    assert!(!*key.0.lock().unwrap(), "device key not destroyed");
    assert!(!rt.notes_status().unwrap().set_up);
}

#[test] fn encrypted_folder_through_the_bridge() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let app = rt.spaces().unwrap()[0].root.clone();
    fs::write(fx.0.join("workspace/doc.txt"), b"top secret report").unwrap();
    fs::create_dir(fx.0.join("workspace/out")).unwrap();
    let handle = rt.vault_create(app.clone(), "Vault".into(), "vault password long enough".into()).unwrap();
    let source = NativeLocation { storage_id: app.storage_id.clone(), path: "doc.txt".into() };
    rt.vault_add(handle.clone(), vec![], source).unwrap();
    rt.vault_mkdir(handle.clone(), vec![], "Папка".into()).unwrap();
    let listing = rt.vault_list(handle.clone(), vec![]).unwrap();
    let names: Vec<(String, bool)> = listing.entries.iter().map(|e| (e.name.clone(), e.folder)).collect();
    assert_eq!(names, [("Папка".to_string(), true), ("doc.txt".to_string(), false)]);
    assert_eq!(listing.damaged, 0);
    // Export decrypts into a new file elsewhere; the spool is gone afterwards.
    let out = NativeLocation { storage_id: app.storage_id.clone(), path: "out".into() };
    rt.vault_export(handle.clone(), vec![], "doc.txt".into(), out.clone()).unwrap();
    assert_eq!(fs::read(fx.0.join("workspace/out/doc.txt")).unwrap(), b"top secret report");
    assert!(rt.vault_export(handle.clone(), vec![], "doc.txt".into(), out).is_err(), "export overwrote a file");
    assert!(!fs::read_dir(fx.0.join("workspace")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")));
    rt.vault_rename(handle.clone(), vec![], "doc.txt".into(), "renamed.txt".into(), false).unwrap();
    rt.vault_delete(handle.clone(), vec![], "renamed.txt".into(), false).unwrap();
    rt.vault_delete(handle.clone(), vec![], "Папка".into(), true).unwrap();
    assert!(rt.vault_list(handle.clone(), vec![]).unwrap().entries.is_empty());
    // No plaintext inside the vault folder.
    fn walk(p: &std::path::Path, out: &mut Vec<PathBuf>) { for e in fs::read_dir(p).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, out) } else { out.push(p) } } }
    let mut files = Vec::new(); walk(&fx.0.join("workspace/Vault"), &mut files);
    for f in files { assert!(!fs::read(&f).unwrap().windows(10).any(|w| w == b"top secret"), "{f:?}"); }
    // Locking forgets the keys; wrong password does not open.
    rt.vault_lock(handle.clone()).unwrap();
    assert!(matches!(rt.vault_list(handle, vec![]), Err(BridgeError::Locked)));
    let root = NativeLocation { storage_id: app.storage_id.clone(), path: "Vault".into() };
    assert!(rt.vault_open(root.clone(), "wrong".into()).is_err());
    let reopened = rt.vault_open(root, "vault password long enough".into()).unwrap();
    assert!(rt.vault_list(reopened, vec![]).is_ok());
    // A spool left by a crash is hidden and removed at the next start.
    fs::write(fx.0.join("workspace/.pnc-spool-00112233445566778899aabbccddeeff"), b"top secret report").unwrap();
    assert!(!rt.list(app.clone()).unwrap().entries.iter().any(|e| e.name.starts_with(".pnc-spool")));
    rt.shutdown().unwrap(); drop(rt);
    let _rt = fx.runtime(vec![]);
    assert!(!fx.0.join("workspace/.pnc-spool-00112233445566778899aabbccddeeff").exists());
}

#[test] fn encrypted_folder_password_recovery_move_and_replace() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    fs::write(fx.0.join("workspace/doc.txt"), b"first version").unwrap();
    fs::write(fx.0.join("workspace/new.txt"), b"second version").unwrap();
    fs::create_dir(fx.0.join("workspace/out")).unwrap();
    let (old, new) = ("vault password long enough", "another password long enough");
    let handle = rt.vault_create(at(""), "Vault".into(), old.into()).unwrap();
    rt.vault_add(handle.clone(), vec![], at("doc.txt")).unwrap();
    rt.vault_mkdir(handle.clone(), vec![], "Папка".into()).unwrap();
    // B12: move into a subfolder; a folder cannot go into itself.
    rt.vault_move(handle.clone(), vec![], "doc.txt".into(), false, vec!["Папка".into()]).unwrap();
    assert_eq!(rt.vault_list(handle.clone(), vec![]).unwrap().entries.len(), 1);
    assert_eq!(rt.vault_list(handle.clone(), vec!["Папка".into()]).unwrap().entries[0].name, "doc.txt");
    assert!(rt.vault_move(handle.clone(), vec![], "Папка".into(), true, vec!["Папка".into()]).is_err());
    // B12: replace with a new version under the same name.
    rt.vault_replace(handle.clone(), vec!["Папка".into()], "doc.txt".into(), at("new.txt")).unwrap();
    rt.vault_export(handle.clone(), vec!["Папка".into()], "doc.txt".into(), at("out")).unwrap();
    assert_eq!(fs::read(fx.0.join("workspace/out/doc.txt")).unwrap(), b"second version");
    // B11: the old password is checked; afterwards only the new one opens.
    assert!(matches!(rt.vault_change_password(handle.clone(), "wrong password here".into(), new.into()), Err(BridgeError::WrongPassword)));
    rt.vault_change_password(handle.clone(), old.into(), new.into()).unwrap();
    assert!(rt.vault_open(at("Vault"), old.into()).is_err());
    let reopened = rt.vault_open(at("Vault"), new.into()).unwrap();
    // B11: recovery key needs the password, opens the vault, a replaced key stops working.
    assert!(rt.vault_set_recovery(reopened.clone(), old.into()).is_err());
    let first = rt.vault_set_recovery(reopened.clone(), new.into()).unwrap();
    let recovered = rt.vault_open_recovery(at("Vault"), first.to_uppercase().replace('-', " ")).unwrap();
    assert_eq!(rt.vault_list(recovered, vec!["Папка".into()]).unwrap().entries[0].name, "doc.txt");
    let second = rt.vault_set_recovery(reopened, new.into()).unwrap();
    assert!(rt.vault_open_recovery(at("Vault"), first).is_err());
    assert!(rt.vault_open_recovery(at("Vault"), second).is_ok());
    assert!(matches!(rt.vault_open_recovery(at("Vault"), "not a key".into()), Err(BridgeError::Invalid)));
    assert!(!fs::read_dir(fx.0.join("workspace")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")));
    rt.shutdown().unwrap();
}

#[test] fn webdav_connection_is_sealed_and_reconnects() {
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-server");
    if !server.exists() { eprintln!("paranoic-server not built: skipped"); return; }
    let fx = Fixture::new();
    let root = fx.0.join("served"); fs::create_dir(&root).unwrap();
    fs::write(root.join("hello.txt"), b"from the server").unwrap();
    let token = "bridge-test-token-0123456789abcdef";
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = fx.0.join("server.json");
    fs::write(&config, format!(r#"{{"root":"{}","listen":"127.0.0.1:{port}","username":"me","token_blake3":"{}"}}"#,
        root.display(), blake3::hash(token.as_bytes()).to_hex())).unwrap();
    struct Kill(std::process::Child);
    impl Drop for Kill { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let _server = Kill(std::process::Command::new(&server).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn().unwrap());
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() { assert!(std::time::Instant::now() < until); std::thread::sleep(Duration::from_millis(50)); }
    let rt = fx.runtime(vec![]);
    assert!(matches!(rt.attach_webdav(format!("http://example.com:{port}"), "me".into(), token.into()), Err(BridgeError::Denied)), "plain http to a remote host");
    let attached = rt.attach_webdav(format!("http://127.0.0.1:{port}"), "me".into(), token.into()).unwrap();
    assert!(!attached.grant.reference.contains(token), "token stored in plain text");
    let listing = rt.list(attached.space.root.clone()).unwrap();
    assert_eq!(listing.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["hello.txt"]);
    rt.shutdown().unwrap(); drop(rt);
    // Next start: saved server is not contacted during start, then reconnects.
    let rt = fx.runtime(vec![attached.grant.clone()]);
    assert!(!rt.spaces().unwrap().iter().any(|s| s.id == attached.space.id));
    let back = rt.reconnect_cloud().unwrap();
    assert_eq!(back.len(), 1);
    assert!(rt.spaces().unwrap().iter().any(|s| s.id == attached.space.id));
    assert_eq!(rt.list(attached.space.root).unwrap().entries.len(), 1);
}

#[test] fn quick_lock_revokes_an_unlock_already_in_progress() {
    use std::sync::{Condvar, atomic::Ordering};
    struct Gate { entered: std::sync::mpsc::SyncSender<()>, released: Mutex<bool>, wake: Condvar }
    impl NotesKeyHost for Gate {
        fn seal(&self, data: Vec<u8>) -> HostBytes { XorSealer.seal(data) }
        fn open(&self, data: Vec<u8>) -> HostBytes {
            self.entered.send(()).unwrap();
            let mut released = self.released.lock().unwrap();
            while !*released { released = self.wake.wait(released).unwrap(); }
            XorSealer.open(data)
        }
        fn erase_key(&self) -> u32 { 0 }
        fn guarantee(&self) -> u8 { 1 }
    }
    let fx = Fixture::new(); let rt = fx.runtime(vec![]);
    let (entered, waiting) = std::sync::mpsc::sync_channel(1);
    let gate = Arc::new(Gate { entered, released: Mutex::new(false), wake: Condvar::new() });
    rt.notes_configure(Some(gate.clone())).unwrap();
    let password = "Correct horse battery staple 42";
    rt.notes_create(password.into()).unwrap(); rt.notes_lock().unwrap();
    let opening = { let rt = rt.clone(); std::thread::spawn(move || rt.notes_unlock(password.into())) };
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let epoch = rt.secrets_epoch.load(Ordering::SeqCst);
    let locking = { let rt = rt.clone(); std::thread::spawn(move || rt.lock_all_secrets()) };
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while rt.secrets_epoch.load(Ordering::SeqCst) == epoch {
        assert!(std::time::Instant::now() < until); std::thread::yield_now();
    }
    *gate.released.lock().unwrap() = true; gate.wake.notify_all();
    assert!(matches!(opening.join().unwrap(), Err(BridgeError::Locked)), "late authentication succeeded after Quick Lock");
    locking.join().unwrap();
    assert!(!rt.notes_status().unwrap().unlocked);
    assert!(matches!(rt.notes_read(), Err(BridgeError::Locked)));
}

/// Plan 1.5 / У-27: quick unlock through the bridge; Clear destroys its key.
#[test] fn notes_quick_unlock_and_clear() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let device = Arc::new(TestKey(Mutex::new(true)));
    let quick = Arc::new(TestKey(Mutex::new(true)));
    rt.notes_configure(Some(device.clone())).unwrap();
    rt.notes_configure_quick(Some(quick.clone())).unwrap();
    let status = rt.notes_status().unwrap();
    assert!(status.quick_available && !status.quick_enabled, "off by default");
    assert!(matches!(rt.notes_configure_quick(None), Err(BridgeError::Busy)), "key changed after the store opened");
    rt.notes_create("Correct horse battery staple 42".into()).unwrap();
    rt.notes_save(book("biometric body")).unwrap();
    rt.notes_quick_enable().unwrap();
    assert!(rt.notes_status().unwrap().quick_enabled);
    rt.notes_lock().unwrap();
    assert!(matches!(rt.notes_quick_enable(), Err(BridgeError::Locked)), "enable needs unlocked notes");
    rt.notes_quick_unlock().unwrap();
    assert_eq!(rt.notes_read().unwrap().tabs[0].notes[0].body, "biometric body");
    // Clear destroys the quick key as well as the device key.
    rt.notes_clear().unwrap();
    assert!(!*device.0.lock().unwrap() && !*quick.0.lock().unwrap(), "a key survived Clear");
    let status = rt.notes_status().unwrap();
    assert!(!status.set_up && !status.quick_enabled);
    assert!(rt.notes_quick_unlock().is_err());
}

/// B4: sync pair through the bridge — plan, run, sealed state across restart.
#[test] fn sync_pair_plans_runs_and_survives_restart() {
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-server");
    if !server.exists() { eprintln!("paranoic-server not built: skipped"); return; }
    let fx = Fixture::new();
    let root = fx.0.join("served"); fs::create_dir(&root).unwrap();
    let token = "bridge-sync-token-0123456789abcdef";
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = fx.0.join("server.json");
    fs::write(&config, format!(r#"{{"root":"{}","listen":"127.0.0.1:{port}","username":"me","token_blake3":"{}"}}"#,
        root.display(), blake3::hash(token.as_bytes()).to_hex())).unwrap();
    struct Kill(std::process::Child);
    impl Drop for Kill { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let _server = Kill(std::process::Command::new(&server).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn().unwrap());
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() { assert!(std::time::Instant::now() < until); std::thread::sleep(Duration::from_millis(50)); }

    let local_dir = fx.0.join("phone"); fs::create_dir(&local_dir).unwrap();
    fs::write(local_dir.join("report-secret.txt"), b"local content").unwrap();
    // Service objects (spools may hold decrypted data) are never synced.
    fs::write(local_dir.join(".pnc-spool-leftover"), b"decrypted secret").unwrap();
    fs::create_dir(local_dir.join(".paranoic-stage")).unwrap();
    fs::write(local_dir.join(".paranoic-stage/part"), b"decrypted secret").unwrap();
    let rt = fx.runtime(vec![]);
    let cloud = rt.attach_webdav(format!("http://127.0.0.1:{port}"), "me".into(), token.into()).unwrap();
    let local_grant = NativeGrant { id: Uuid::new_v4().to_string(), reference: local_dir.to_string_lossy().into_owned(), android_tree: false };
    let local = rt.attach(local_grant.clone()).unwrap();
    assert!(rt.sync_pair_add(cloud.space.root.clone(), local.root.clone(), 2, false).is_err(), "cloud accepted as the local side");
    let app = rt.spaces().unwrap()[0].root.clone();
    assert!(matches!(rt.sync_pair_add(app, cloud.space.root.clone(), 2, false), Err(BridgeError::Denied)), "app workspace accepted as a sync side");
    let pair = rt.sync_pair_add(local.root.clone(), cloud.space.root.clone(), 2, false).unwrap();
    let plan = rt.sync_plan(pair.id.clone()).unwrap();
    assert!(plan.actions.iter().any(|a| a.path == "report-secret.txt" && a.kind == 3), "upload not planned");
    let result = rt.sync_run(pair.id.clone()).unwrap();
    assert_eq!((result.done, result.failed), (1, 0));
    assert_eq!(fs::read(root.join("report-secret.txt")).unwrap(), b"local content");
    assert!(!root.join(".pnc-spool-leftover").exists() && !root.join(".paranoic-stage").exists(), "service objects uploaded");
    assert!(plan.actions.iter().all(|a| !a.path.contains(".pnc-") && !a.path.contains(".paranoic-")), "service objects planned");
    let state = fs::read(fx.0.join("state/sync-state")).unwrap();
    assert!(!state.windows(13).any(|w| w == b"report-secret"), "sync state stores names in clear");
    rt.shutdown().unwrap(); drop(rt);

    // Next start: the sealed pair and its baseline are back; nothing left to do.
    let rt = fx.runtime(vec![cloud.grant.clone(), local_grant]);
    rt.reconnect_cloud().unwrap();
    let pairs = rt.sync_pairs().unwrap();
    assert_eq!(pairs.len(), 1);
    let quiet = rt.sync_plan(pairs[0].id.clone()).unwrap();
    assert!(quiet.actions.iter().all(|a| a.kind == 9), "work planned after a completed sync: {:?}", quiet.actions.iter().map(|a| (&a.path, a.kind)).collect::<Vec<_>>());
    rt.sync_pair_remove(pairs[0].id.clone()).unwrap();
    assert!(rt.sync_pairs().unwrap().is_empty());
}

/// External audit (2026-11-14): only exact spool names are swept at start; an
/// operation that started before Quick Lock does not publish afterwards.
#[test] fn strict_spool_names_and_no_publication_after_quick_lock() {
    assert!(crate::features::is_spool(".pnc-spool-0123456789abcdef0123456789abcdef"));
    assert!(crate::features::is_spool(".pnc-spool-0123456789abcdef0123456789abcdef-m"));
    assert!(!crate::features::is_spool(".pnc-spool-my notes"), "a user name with the prefix is swept");
    assert!(!crate::features::is_spool(".pnc-spool-"));
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let app = rt.spaces().unwrap()[0].root.clone();
    let handle = rt.vault_create(app, "Vault".into(), "vault password long enough".into()).unwrap();
    let vault = rt.vault(&handle).unwrap();
    assert!(rt.still_open(&handle, &vault).is_ok());
    rt.lock_all_secrets();
    assert!(matches!(rt.still_open(&handle, &vault), Err(BridgeError::Locked)), "work started before Quick Lock may still publish");
    rt.shutdown().unwrap();
}

/// Plan 5.1: the personal folder opens with the notes session, is hidden from
/// the app's file list, survives a restart, needs the notes unlocked, and
/// «Очистить всё» leaves nothing that opens.
#[test] fn safe_folder_opens_with_the_notes_and_dies_with_clear() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(TestKey(Mutex::new(true))))).unwrap();
    assert!(matches!(rt.safe_open(), Err(BridgeError::NotSetUp | BridgeError::Locked)));
    let password = "Correct horse battery staple 42";
    rt.notes_create(password.into()).unwrap();
    let ws = fx.0.join("workspace");
    fs::write(ws.join("photo.txt"), "private marker").unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let handle = rt.safe_open().unwrap();
    rt.vault_add(handle.clone(), vec![], at("photo.txt")).unwrap();
    assert_eq!(rt.vault_list(handle.clone(), vec![]).unwrap().entries.single_name(), "photo.txt");
    let listed: Vec<String> = rt.list(app.clone()).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert!(!listed.iter().any(|n| n.contains("safe")), "personal folder visible in Files: {listed:?}");
    let raw: Vec<u8> = walk(&ws.join(".pnc-safe")).into_iter().flat_map(|p| fs::read(p).unwrap()).collect();
    assert!(!raw.windows(14).any(|w| w == b"private marker"), "content stored in the clear");

    // Locking the notes ends the folder too; it reopens after unlock, also after a restart.
    rt.notes_lock().unwrap();
    rt.lock_secrets();
    assert!(matches!(rt.vault_list(handle, vec![]), Err(BridgeError::Locked)));
    assert!(matches!(rt.safe_open(), Err(BridgeError::Locked)));
    rt.shutdown().unwrap(); drop(rt);
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(TestKey(Mutex::new(true))))).unwrap();
    rt.notes_unlock(password.into()).unwrap();
    let handle = rt.safe_open().unwrap();
    assert_eq!(rt.vault_list(handle, vec![]).unwrap().entries.single_name(), "photo.txt");

    // «Очистить всё»: notes and folder gone; a new Safe starts empty.
    rt.safe_clear().unwrap();
    assert!(!ws.join(".pnc-safe").exists(), "encrypted folder left");
    assert!(!rt.notes_status().unwrap().set_up);
    // Clear destroyed the device key; the app starts again with a new one.
    rt.shutdown().unwrap(); drop(rt);
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(TestKey(Mutex::new(true))))).unwrap();
    rt.notes_create(password.into()).unwrap();
    let handle = rt.safe_open().unwrap();
    assert!(rt.vault_list(handle, vec![]).unwrap().entries.is_empty());
    rt.shutdown().unwrap();
}
trait SingleName { fn single_name(&self) -> String; }
impl SingleName for Vec<NativeVaultEntry> { fn single_name(&self) -> String { assert_eq!(self.len(), 1, "{}", self.len()); self[0].name.clone() } }
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap() { let p = e.unwrap().path(); if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) } }
    out
}

/// Like the Android Keystore key: erasing it makes old seals unopenable, and
/// the next seal creates a new key.
struct RegenKey(Mutex<u8>);
impl NotesKeyHost for RegenKey {
    fn seal(&self, data: Vec<u8>) -> HostBytes { let g = *self.0.lock().unwrap(); HostBytes { error: 0, data: [vec![g], data.iter().map(|b| b ^ g).collect()].concat() } }
    fn open(&self, data: Vec<u8>) -> HostBytes {
        let g = *self.0.lock().unwrap();
        if data.first() != Some(&g) { return HostBytes { error: 1, data: vec![] }; }
        HostBytes { error: 0, data: data[1..].iter().map(|b| b ^ g).collect() }
    }
    fn erase_key(&self) -> u32 { *self.0.lock().unwrap() += 1; 0 }
    fn guarantee(&self) -> u8 { 2 }
}

/// Plan 5.2 through the bridge: the duress password unlocks "normally" into
/// empty notes, and the personal folder is gone without a message.
#[test] fn duress_password_through_the_bridge_removes_the_safe_silently() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(RegenKey(Mutex::new(0x51))))).unwrap();
    let (password, duress) = ("Correct horse battery staple 42", "Quiet meadow under cold stars 7");
    rt.notes_create(password.into()).unwrap();
    rt.notes_save(book("real secrets")).unwrap();
    let ws = fx.0.join("workspace");
    fs::write(ws.join("doc.txt"), "private marker").unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let handle = rt.safe_open().unwrap();
    rt.vault_add(handle, vec![], NativeLocation { storage_id: app.storage_id.clone(), path: "doc.txt".into() }).unwrap();
    assert!(matches!(rt.notes_set_duress(Some(password.into())), Err(BridgeError::Invalid)));
    rt.notes_set_duress(Some(duress.into())).unwrap();
    assert!(rt.notes_duress_enabled().unwrap());
    rt.notes_lock().unwrap();
    rt.notes_unlock(duress.into()).unwrap(); // looks like an ordinary unlock
    assert!(rt.notes_read().unwrap().tabs.is_empty(), "notes survived");
    assert!(!ws.join(".pnc-safe").exists(), "personal folder survived");
    assert!(!rt.notes_duress_enabled().unwrap());
    let handle = rt.safe_open().unwrap();
    assert!(rt.vault_list(handle, vec![]).unwrap().entries.is_empty(), "a new Safe is not empty");
    rt.shutdown().unwrap();
}

/// Batch 1 of the integration seams through the bridge: large text rows,
/// notes export → import, recovery challenge and reset, password helpers.
#[test] fn integration_batch_one_calls() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(TestKey(Mutex::new(true))))).unwrap();
    let ws = fx.0.join("workspace");
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };

    // E2.6
    let text: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    fs::write(ws.join("big.txt"), &text).unwrap();
    let pager = rt.text_pager_open(at("big.txt")).unwrap();
    assert_eq!(pager.rows, 5000);
    assert_eq!(rt.text_pager_read(pager.handle.clone(), 4998, 10).unwrap(), ["line 4998", "line 4999"]);
    fs::write(ws.join("big.txt"), "changed").unwrap();
    assert!(matches!(rt.text_pager_read(pager.handle.clone(), 0, 1), Err(BridgeError::Changed)));
    rt.text_pager_close(pager.handle.clone());
    assert!(matches!(rt.text_pager_read(pager.handle, 0, 1), Err(BridgeError::Invalid)));

    // E5.3
    let password = "Correct horse battery staple 42";
    rt.notes_create(password.into()).unwrap();
    rt.notes_save(book("first note body")).unwrap();
    let exported = rt.notes_export(at(""), "Notes export".into()).unwrap();
    assert_eq!((exported.files, exported.skipped_attachments), (1, 0));
    assert_eq!(fs::read_to_string(ws.join("Notes export/Ключи/seed.md")).unwrap(), "first note body");
    assert!(matches!(rt.notes_export(at(""), "Notes export".into()), Err(BridgeError::Conflict)), "merged into an existing folder");
    fs::write(ws.join("loose.txt"), "loose text").unwrap();
    let imported = rt.notes_import(vec![at("Notes export"), at("loose.txt")], "Импорт".into(), None).unwrap();
    assert_eq!((imported.notes, imported.skipped), (2, 0));
    let tabs: Vec<String> = rt.notes_read().unwrap().tabs.into_iter().map(|t| t.title).collect();
    assert_eq!(tabs, ["Ключи", "Ключи", "Импорт"]);
    assert!(!fs::read_dir(&ws).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")), "spool left");

    // E5.4
    fs::write(ws.join("doc.txt"), "vault content").unwrap();
    let handle = rt.vault_create(at(""), "Vault".into(), password.into()).unwrap();
    rt.vault_add(handle.clone(), vec![], at("doc.txt")).unwrap();
    let recovery = rt.vault_set_recovery(handle.clone(), password.into()).unwrap();
    let picked = rt.vault_recovery_challenge(3).unwrap();
    let groups: Vec<&str> = recovery.split('-').collect();
    let answers: Vec<String> = picked.iter().map(|&i| groups[i as usize].to_uppercase()).collect();
    assert!(rt.vault_recovery_check(recovery.clone(), picked.clone(), answers));
    assert!(!rt.vault_recovery_check(recovery.clone(), picked, vec!["xxxx".into(), "yyyy".into(), "zzzz".into()]));
    rt.vault_lock(handle).unwrap();
    let opened = rt.vault_open_recovery(at("Vault"), recovery.clone()).unwrap();
    rt.vault_reset_password(opened.clone(), recovery, "brand new vault passphrase 77".into()).unwrap();
    rt.vault_lock(opened).unwrap();
    assert!(matches!(rt.vault_open(at("Vault"), password.into()), Err(BridgeError::WrongPassword)));
    rt.vault_open(at("Vault"), "brand new vault passphrase 77".into()).unwrap();

    // E6.1
    let weak = rt.password_feedback("abc".into());
    assert_eq!(weak.strength, 0);
    assert!(weak.hints.contains(&1) && weak.hints.contains(&6));
    let phrase = rt.generate_passphrase(6).unwrap();
    assert_eq!(rt.password_feedback(phrase).strength, 2, "generated passwords satisfy the rule");
    // Owner's rule: lowercase words alone miss uppercase/digit, so a raw phrase is weak.
    assert_eq!(rt.password_feedback("Xq7#vRt9-Mzw2".into()).strength, 2);
    // Audit 16: a run ("bcdefgh") keeps it acceptable but not strong.
    assert_eq!(rt.password_feedback("Abcdefgh1!xy".into()).strength, 1);
    assert!(rt.generate_passphrase(2).is_err());
    rt.shutdown().unwrap();
}

/// У-68: notes erased without the folder (auto-wipe path, interrupted
/// «Очистить всё»): the old personal folder cannot be read with the new
/// secret; the next Safe starts empty instead of failing.
#[test] fn safe_left_behind_by_a_wipe_is_replaced_not_reopened() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    rt.notes_configure(Some(Arc::new(RegenKey(Mutex::new(0x33))))).unwrap();
    let password = "Correct horse battery staple 42";
    rt.notes_create(password.into()).unwrap();
    let ws = fx.0.join("workspace");
    fs::write(ws.join("doc.txt"), "old private").unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let handle = rt.safe_open().unwrap();
    rt.vault_add(handle, vec![], NativeLocation { storage_id: app.storage_id.clone(), path: "doc.txt".into() }).unwrap();
    // Notes erased, folder left (as after auto-wipe in older code or a failed removal).
    rt.notes_clear().unwrap();
    assert!(ws.join(".pnc-safe").exists());
    rt.notes_create(password.into()).unwrap();
    let handle = rt.safe_open().unwrap();
    assert!(rt.vault_list(handle, vec![]).unwrap().entries.is_empty(), "old folder reopened or kept");
    rt.shutdown().unwrap();
}

/// Move into a vault: verified copy first, then only that source version is
/// deleted; a repeat after a lost reply finishes the job; a changed source is
/// kept. Share into a vault from a stream with an exact size.
#[test] fn move_into_vault_and_add_from_a_stream() {
    struct Bytes(Mutex<Vec<u8>>);
    impl crate::IncomingStream for Bytes {
        fn read(&self, max: u32) -> HostBytes {
            let mut data = self.0.lock().unwrap();
            let n = (max as usize).min(data.len());
            HostBytes { error: 0, data: data.drain(..n).collect() }
        }
    }
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let ws = fx.0.join("workspace");
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let handle = rt.vault_create(at(""), "Vault".into(), "Correct horse battery staple 42".into()).unwrap();
    fs::write(ws.join("photo.jpg"), vec![7u8; 200_000]).unwrap();
    let moved = rt.vault_move_in(handle.clone(), vec![], at("photo.jpg"), Uuid::new_v4().to_string()).unwrap();
    assert!(moved.source_removed && moved.bytes == 200_000);
    assert!(!ws.join("photo.jpg").exists());
    // A lost reply: the same file again (as if the delete had not happened) finishes without a second copy.
    fs::write(ws.join("doc.txt"), "same content").unwrap();
    rt.vault_add(handle.clone(), vec![], at("doc.txt")).unwrap();
    let again = rt.vault_move_in(handle.clone(), vec![], at("doc.txt"), Uuid::new_v4().to_string()).unwrap();
    assert!(again.source_removed && !ws.join("doc.txt").exists());
    // A different file of the same name is never treated as already moved.
    fs::write(ws.join("doc.txt"), "different content").unwrap();
    assert!(matches!(rt.vault_move_in(handle.clone(), vec![], at("doc.txt"), Uuid::new_v4().to_string()), Err(BridgeError::Conflict)));
    assert!(ws.join("doc.txt").exists(), "source deleted on a conflict");
    assert_eq!(rt.vault_list(handle.clone(), vec![]).unwrap().entries.len(), 2);
    // У-69: the source delete is bound to the verified content. Same size and
    // time but different bytes (or another object under the name) is kept.
    fs::write(ws.join("aba.bin"), "original bytes!").unwrap();
    let source = paranoic_core::Location { storage: StorageId(Uuid::parse_str(&app.storage_id).unwrap()), path: ResourcePath::parse("aba.bin").unwrap() };
    let meta = rt.core.metadata(&source).unwrap();
    let digest = *blake3::hash(b"original bytes!").as_bytes();
    let stamp = fs::metadata(ws.join("aba.bin")).unwrap().modified().unwrap();
    fs::write(ws.join("aba.bin"), "replaced bytes!").unwrap();
    fs::File::options().write(true).open(ws.join("aba.bin")).unwrap().set_modified(stamp).unwrap();
    assert_eq!(rt.core.metadata(&source).unwrap(), meta, "forged metadata");
    let job = rt.core.submit_with_id(JobId(Uuid::new_v4()),
        Operation::DeleteFileContent { target: source.clone(), expected: meta.clone(), expected_blake3: digest }).unwrap();
    assert!(crate::features::run_wait(&rt.core, job.id).and_then(|r| crate::features::finish(r, 1)).is_err());
    assert_eq!(fs::read_to_string(ws.join("aba.bin")).unwrap(), "replaced bytes!", "a different version was deleted");
    let job = rt.core.submit_with_id(JobId(Uuid::new_v4()),
        Operation::DeleteFileContent { target: source, expected: meta, expected_blake3: *blake3::hash(b"replaced bytes!").as_bytes() }).unwrap();
    crate::features::finish(crate::features::run_wait(&rt.core, job.id).unwrap(), 1).unwrap();
    assert!(!ws.join("aba.bin").exists());

    // Share: exact size accepted; short or long refused, nothing added.
    rt.vault_add_stream(handle.clone(), vec![], "shared.bin".into(), 5, Some(1), Arc::new(Bytes(Mutex::new(b"hello".to_vec())))).unwrap();
    assert!(rt.vault_add_stream(handle.clone(), vec![], "short.bin".into(), 9, None, Arc::new(Bytes(Mutex::new(b"hello".to_vec())))).is_err());
    assert!(rt.vault_add_stream(handle.clone(), vec![], "long.bin".into(), 3, None, Arc::new(Bytes(Mutex::new(b"hello".to_vec())))).is_err());
    let names: Vec<String> = rt.vault_list(handle.clone(), vec![]).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    rt.vault_export(handle, vec![], "shared.bin".into(), at("")).unwrap();
    assert_eq!(fs::read(ws.join("shared.bin")).unwrap(), b"hello");
    assert!(!fs::read_dir(&ws).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")));
    rt.shutdown().unwrap();
}

/// E4.7: inside a vault — random-access stream, text open/save/create with a
/// version check, a ZIP listed and extracted into the vault; plaintext never
/// lands in a file; Quick Lock ends open streams.
#[test] fn work_inside_a_vault_without_plaintext_files() {
    let fx = Fixture::new();
    let rt = fx.runtime(vec![]);
    let ws = fx.0.join("workspace");
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let handle = rt.vault_create(at(""), "Vault".into(), "Correct horse battery staple 42".into()).unwrap();
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(ws.join("movie.bin"), &big).unwrap();
    rt.vault_move_in(handle.clone(), vec![], at("movie.bin"), Uuid::new_v4().to_string()).unwrap();
    // Stream.
    let stream = rt.vault_stream_open(handle.clone(), vec![], "movie.bin".into()).unwrap();
    assert_eq!(stream.size, 300_000);
    assert_eq!(rt.vault_stream_read(stream.id.clone(), 131_070, 10).unwrap(), big[131_070..131_080]);
    assert_eq!(rt.vault_stream_read(stream.id.clone(), 299_995, 100).unwrap(), big[299_995..]);
    // Text.
    rt.vault_text_create(handle.clone(), vec![], "note.md".into(), "# Тайна\nстрока".into()).unwrap();
    assert!(matches!(rt.vault_text_create(handle.clone(), vec![], "note.md".into(), "x".into()), Err(BridgeError::Conflict)));
    let text = rt.vault_text_open(handle.clone(), vec![], "note.md".into()).unwrap();
    assert_eq!(text.text, "# Тайна\nстрока");
    rt.vault_text_save(handle.clone(), vec![], "note.md".into(), text.version_token.clone(), "# Тайна\nновая строка".into()).unwrap();
    assert!(matches!(rt.vault_text_save(handle.clone(), vec![], "note.md".into(), text.version_token.clone(), "stale".into()),
        Err(BridgeError::Changed)), "saved over a newer version");
    assert_eq!(rt.vault_text_open(handle.clone(), vec![], "note.md".into()).unwrap().text, "# Тайна\nновая строка");
    // У-70: another write with the SAME encrypted size and time is still a
    // different version — a stale editor must not overwrite it.
    let opened = rt.vault_text_open(handle.clone(), vec![], "note.md".into()).unwrap();
    let size = opened.version_bytes;
    let object = || -> std::path::PathBuf {
        fn files(d: &std::path::Path, out: &mut Vec<PathBuf>) { for e in fs::read_dir(d).unwrap() { let p = e.unwrap().path(); if p.is_dir() { files(&p, out) } else { out.push(p) } } }
        let mut found = Vec::new(); files(&ws.join("Vault"), &mut found);
        found.into_iter().filter(|p| fs::metadata(p).unwrap().len() == size).max_by_key(|p| fs::metadata(p).unwrap().modified().unwrap()).unwrap()
    };
    let before = object();
    let stamp = fs::metadata(&before).unwrap().modified().unwrap();
    rt.vault_text_save(handle.clone(), vec![], "note.md".into(), opened.version_token.clone(), "# Тайна\nновая строкА".into()).unwrap();
    let after = object();
    assert_eq!(fs::metadata(&after).unwrap().len(), opened.version_bytes);
    fs::File::options().write(true).open(&after).unwrap().set_modified(stamp).unwrap();
    let forged = rt.vault_text_open(handle.clone(), vec![], "note.md".into()).unwrap();
    assert_eq!((forged.version_bytes, forged.version_modified_ms), (opened.version_bytes, opened.version_modified_ms));
    assert_ne!(forged.version_token, opened.version_token);
    assert!(matches!(rt.vault_text_save(handle.clone(), vec![], "note.md".into(), opened.version_token.clone(), "stale".into()),
        Err(BridgeError::Changed)), "same size and time must not pass for the same version");
    assert_eq!(rt.vault_text_open(handle.clone(), vec![], "note.md".into()).unwrap().text, "# Тайна\nновая строкА");
    // ZIP inside the vault.
    fs::create_dir_all(ws.join("src/inner")).unwrap();
    fs::write(ws.join("src/a.txt"), "archived secret").unwrap();
    fs::write(ws.join("src/inner/b.txt"), "deeper secret").unwrap();
    rt.archive_create(vec![at("src")], ARCHIVE_ZIP, Some("zip password long".into()), at(""), "pack.zip".into()).unwrap();
    fs::remove_dir_all(ws.join("src")).unwrap();
    rt.vault_move_in(handle.clone(), vec![], at("pack.zip"), Uuid::new_v4().to_string()).unwrap();
    let listed = rt.vault_archive_list(handle.clone(), vec![], "pack.zip".into(), ARCHIVE_ZIP, None).unwrap();
    assert!(listed.iter().any(|e| e.path == "src/inner/b.txt" && e.encrypted));
    assert!(matches!(rt.vault_archive_extract(handle.clone(), vec![], "pack.zip".into(), ARCHIVE_ZIP, None, vec![], "out".into()), Err(BridgeError::PasswordRequired)));
    // Audit 24: a wrong password changes nothing, and the same name is free for the retry.
    assert!(matches!(rt.vault_archive_extract(handle.clone(), vec![], "pack.zip".into(), ARCHIVE_ZIP, Some("wrong password!!".into()), vec![], "out".into()),
        Err(BridgeError::WrongPassword)));
    assert!(!rt.vault_list(handle.clone(), vec![]).unwrap().entries.iter().any(|e| e.name == "out"), "wrong password left a folder in the vault");
    assert_eq!(rt.vault_archive_extract(handle.clone(), vec![], "pack.zip".into(), ARCHIVE_ZIP, Some("zip password long".into()), vec![], "out".into()).unwrap(), 2);
    let inner = rt.vault_text_open(handle.clone(), vec!["out".into(), "src".into(), "inner".into()], "b.txt".into()).unwrap();
    assert_eq!(inner.text, "deeper secret");
    // No plaintext anywhere in the app folder.
    fn walk(dir: &std::path::Path, out: &mut Vec<u8>) {
        for e in fs::read_dir(dir).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, out) } else { out.extend(fs::read(p).unwrap()) } }
    }
    let mut all = Vec::new(); walk(&ws, &mut all);
    for secret in [&b"archived secret"[..], b"deeper secret", "новая строка".as_bytes()] {
        assert!(!all.windows(secret.len()).any(|w| w == secret), "plaintext found on disk");
    }
    // Quick Lock ends streams.
    rt.lock_secrets();
    assert!(matches!(rt.vault_stream_read(stream.id, 0, 10), Err(BridgeError::Locked)));
    rt.shutdown().unwrap();
}
