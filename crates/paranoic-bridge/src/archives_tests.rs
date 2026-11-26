//! Plan 2.1: ZIP (AES) and 7z through NativeRuntime — create, list, extract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf};

use crate::test_support::{created, info, NoDocs, NoSealer};
/// The archive source I/O pool is process-wide (2 slots, bounded admission
/// wait): archive tests must not compete for it in parallel.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|e| e.into_inner()) }
struct Fixture(PathBuf);
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn tree(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if p.is_dir() { out.push((format!("{rel}/"), vec![])); walk(base, &p, out) } else { out.push((rel, fs::read(&p).unwrap())) }
        }
    }
    let mut out = Vec::new(); walk(root, root, &mut out); out.sort(); out
}
fn no_spool(ws: &std::path::Path) -> bool { !fs::read_dir(ws).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")) }
fn spool_names(ws: &std::path::Path) -> std::collections::BTreeSet<String> {
    fs::read_dir(ws).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".pnc-spool")).collect()
}

#[test] fn archive_enumeration_does_not_keep_source_descriptors_open() {
    let _serial = serial();
    use std::io::Read;
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-lazy-source-{}", Uuid::new_v4())));
    fs::create_dir_all(&fx.0).unwrap();
    let storage = Arc::new(LocalStorage::open(StorageId(Uuid::new_v4()), &fx.0, Access::ReadWrite).unwrap());
    let path = ResourcePath::parse("source").unwrap();
    fs::write(fx.0.join("source"), b"original").unwrap();
    let expected = storage.metadata(&path).unwrap();
    let mut deferred = crate::archives::DeferredArchiveSource::new(storage, path, expected);
    // An already-open descriptor would still read the unlinked old bytes.
    fs::remove_file(fx.0.join("source")).unwrap();
    assert!(deferred.read_to_end(&mut Vec::new()).is_err());
}

/// A lost publication reply must retain its input, including across restart.
#[test] fn journal_referenced_spool_survives_guard_drop_and_restart() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-spool-{}", Uuid::new_v4())));
    let ws = fx.0.join("workspace");
    fs::create_dir_all(&ws).unwrap();
    let open = || NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let rt = open();
    let mut held = crate::spool::PublicationSpool::new(&rt).unwrap();
    let path = held.path().clone();
    fs::write(ws.join(path.as_str()), b"prepared content").unwrap();
    rt.core.submit(Operation::CopyFile { source: held.handoff(),
        target: Location { storage: rt.app.id(), path: ResourcePath::parse("copy").unwrap() },
        conflict: paranoic_core::ConflictPolicy::Fail }).unwrap();
    drop(held); // no completed acknowledgement, regardless of the worker's speed
    assert!(ws.join(path.as_str()).exists());
    fs::write(ws.join(".pnc-spool-0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"), b"orphan").unwrap();
    rt.shutdown().unwrap(); drop(rt);
    let rt = open();
    assert_eq!(fs::read(ws.join(path.as_str())).unwrap(), b"prepared content");
    assert!(!ws.join(".pnc-spool-0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f").exists());
    rt.shutdown().unwrap();
}

#[test] fn zip_and_7z_create_list_extract() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-archives-{}", Uuid::new_v4())));
    for d in ["state", "workspace/src/docs/sub"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let ws = fx.0.join("workspace");
    fs::write(ws.join("src/docs/a.txt"), "привет, архив").unwrap();
    fs::write(ws.join("src/docs/sub/b.bin"), (0..300_000u32).flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    fs::write(ws.join("src/c.txt"), "top level").unwrap();
    let worker = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-pdf-worker");
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), worker.to_str().unwrap().into(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let sources = || vec![at("src/docs"), at("src/c.txt")];
    let expected = { let mut t = tree(&ws.join("src")); t.sort(); t };

    // ZIP with AES password.
    rt.archive_create(sources(), ARCHIVE_ZIP, Some("zip password".into()), at(""), "pack.zip".into()).unwrap();
    assert!(no_spool(&ws), "successful creation retained its spool");
    assert!(matches!(rt.archive_create(sources(), ARCHIVE_ZIP, None, at(""), "pack.zip".into()), Err(BridgeError::Conflict)));
    let retained_after_conflict = spool_names(&ws);
    assert_eq!(retained_after_conflict.len(), 1, "failed publication must retain its input");
    assert!(retained_after_conflict.is_subset(&crate::spool::referenced(&rt.core, rt.app.id()).unwrap()));
    let mut listed: Vec<String> = rt.archive_list(at("pack.zip"), ARCHIVE_ZIP, None).unwrap().into_iter().map(|e| e.path).collect();
    listed.sort();
    assert_eq!(listed, ["c.txt", "docs", "docs/a.txt", "docs/sub", "docs/sub/b.bin"]);
    assert!(rt.archive_extract(at("pack.zip"), ARCHIVE_ZIP, Some("wrong".into()), at(""), "bad".into()).is_err());
    assert!(!ws.join("bad").exists(), "partial extraction published");
    rt.archive_extract(at("pack.zip"), ARCHIVE_ZIP, Some("zip password".into()), at(""), "unzipped".into()).unwrap();
    assert_eq!(tree(&ws.join("unzipped")), expected);
    assert!(matches!(rt.archive_extract(at("pack.zip"), ARCHIVE_ZIP, Some("zip password".into()), at(""), "unzipped".into()),
        Err(BridgeError::Conflict)), "merged into an existing folder");
    // Into another storage (a connected folder): Core checks the manifest during the move.
    fs::create_dir_all(fx.0.join("external")).unwrap();
    let ext = rt.attach(NativeGrant { id: Uuid::new_v4().to_string(), reference: fx.0.join("external").to_str().unwrap().into(), android_tree: false }).unwrap();
    rt.archive_extract(at("pack.zip"), ARCHIVE_ZIP, Some("zip password".into()), ext.root.clone(), "fromzip".into()).unwrap();
    assert_eq!(tree(&fx.0.join("external/fromzip")), expected);

    // 7z: decoding only in the confined worker.
    if worker.exists() {
        assert!(matches!(rt.archive_create(sources(), ARCHIVE_7Z, Some("x".into()), at(""), "p.7z".into()), Err(BridgeError::Unsupported)));
        rt.archive_create(sources(), ARCHIVE_7Z, None, at(""), "pack.7z".into()).unwrap();
        assert_eq!(rt.archive_list(at("pack.7z"), ARCHIVE_7Z, None).unwrap().len(), 5);
        rt.archive_extract(at("pack.7z"), ARCHIVE_7Z, None, at(""), "un7z".into()).unwrap();
        assert_eq!(tree(&ws.join("un7z")), expected);
    } else { eprintln!("paranoic-pdf-worker not built: 7z part skipped"); }
    assert!(matches!(rt.archive_list(at("pack.zip"), 9, None), Err(BridgeError::Invalid)));
    assert_eq!(spool_names(&ws), retained_after_conflict,
        "successful archive operations leaked another spool or lost failed-job input");
    // A spool folder left by a crash (decrypted content) is removed at the next start.
    fs::create_dir_all(ws.join(".pnc-spool-c0ffee00c0ffee00c0ffee00c0ffee00/inner")).unwrap();
    fs::write(ws.join(".pnc-spool-c0ffee00c0ffee00c0ffee00c0ffee00/inner/secret.txt"), "plaintext").unwrap();
    rt.shutdown().unwrap(); drop(rt);
    let _rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    assert!(!ws.join(".pnc-spool-c0ffee00c0ffee00c0ffee00c0ffee00").exists(), "crashed spool folder kept");
}


/// A pipe-backed provider stream (some cloud documents): bytes only in order;
/// any other offset is refused like the Android host does (code 2).
struct PipeDocs { data: Vec<u8>, position: Mutex<u64>, opened: Mutex<u32> }
impl PipeDocs {
    fn file(&self) -> HostInfo { HostInfo { error: 0, kind: 1, bytes: self.data.len() as u64, modified_ms: Some(1000) } }
}
impl AndroidDocuments for PipeDocs {
    fn metadata(&self, _: String, _: String) -> HostInfo { self.file() }
    fn list(&self, _: String, _: String, _: u32) -> HostListing { HostListing { error: 2, entries: vec![] } }
    fn open(&self, _: String, _: String) -> HostOpened {
        *self.position.lock().unwrap() = 0; *self.opened.lock().unwrap() += 1;
        HostOpened { error: 0, id: 1, info: self.file() }
    }
    fn read(&self, _: u64, offset: u64, size: u32) -> HostBytes {
        let mut position = self.position.lock().unwrap();
        if offset != *position { return HostBytes { error: 2, data: vec![] }; }
        let start = offset as usize;
        let end = (start + size as usize).min(self.data.len());
        *position = end as u64;
        HostBytes { error: 0, data: self.data[start..end].to_vec() }
    }
    fn file_metadata(&self, _: u64) -> HostInfo { self.file() }
    fn close(&self, _: u64) {}
    fn writable(&self, _: String) -> bool { false }
    fn create(&self, _: String, _: String, _: String, _: bool) -> HostCreated { created() }
    fn open_write(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
    fn write(&self, _: u64, _: Vec<u8>) -> u32 { 2 }
    fn finish_write(&self, _: u64) -> u32 { 2 }
    fn rename(&self, _: String, _: String, _: String) -> HostCreated { created() }
    fn delete(&self, _: String, _: String, _: bool) -> u32 { 2 }
    fn resolve(&self, _: String, _: String) -> HostCreated { created() }
    fn move_document(&self, _: String, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
}

/// У-61: an archive from a stream without random access is read through a
/// private copy, and the copy is removed afterwards.
#[test] fn archive_from_a_sequential_provider_stream_lists_and_extracts() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-archive-pipe-{}", Uuid::new_v4())));
    for d in ["state", "workspace/src"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    let ws = fx.0.join("workspace");
    fs::write(ws.join("src/a.txt"), "из потока").unwrap();
    fs::write(ws.join("src/b.bin"), (0..400_000u32).flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    rt.archive_create(vec![at("src")], ARCHIVE_ZIP, Some("stream password".into()), at(""), "pack.zip".into()).unwrap();
    let docs = Arc::new(PipeDocs { data: fs::read(ws.join("pack.zip")).unwrap(), position: Mutex::new(0), opened: Mutex::new(0) });
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()), tree: "content://fixture.invalid/tree/pipe".into(),
        host: docs.clone(), writable: false };
    let remote = NativeLocation { storage_id: saf.id.0.to_string(), path: "pack.zip".into() };
    rt.core.register_storage(Arc::new(saf)).unwrap();

    let mut listed: Vec<String> = rt.archive_list(remote.clone(), ARCHIVE_ZIP, None).unwrap().into_iter().map(|e| e.path).collect();
    listed.sort();
    assert_eq!(listed, ["src", "src/a.txt", "src/b.bin"]);
    assert_eq!(*docs.opened.lock().unwrap(), 2, "probe, then one sequential copy");
    assert!(no_spool(&ws), "private copy of the archive left behind after listing");
    rt.archive_extract(remote.clone(), ARCHIVE_ZIP, Some("stream password".into()), at(""), "out".into()).unwrap();
    assert_eq!(tree(&ws.join("out/src")), tree(&ws.join("src")));
    assert!(no_spool(&ws), "private copy of the archive left behind after extraction");
    // A wrong password still fails and still removes the copy.
    assert!(rt.archive_extract(remote, ARCHIVE_ZIP, Some("wrong".into()), at(""), "bad".into()).is_err());
    assert!(no_spool(&ws) && !ws.join("bad").exists());
}

/// E2.1 at the bridge: encrypted entries are marked; a missing password and a
/// wrong one reach the host as PasswordRequired / WrongPassword.
#[test] fn archive_password_outcomes_reach_the_host() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-archive-pw-{}", Uuid::new_v4())));
    for d in ["state", "workspace/src"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    let ws = fx.0.join("workspace");
    fs::write(ws.join("src/a.txt"), "secret").unwrap();
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    rt.archive_create(vec![at("src")], ARCHIVE_ZIP, Some("archive password".into()), at(""), "p.zip".into()).unwrap();
    let listed = rt.archive_list(at("p.zip"), ARCHIVE_ZIP, None).unwrap();
    assert!(listed.iter().all(|e| e.encrypted != e.folder), "files encrypted, folders not");
    assert!(matches!(rt.archive_extract(at("p.zip"), ARCHIVE_ZIP, None, at(""), "a".into()), Err(BridgeError::PasswordRequired)));
    assert!(matches!(rt.archive_extract(at("p.zip"), ARCHIVE_ZIP, Some("wrong".into()), at(""), "b".into()), Err(BridgeError::WrongPassword)));
    rt.archive_extract(at("p.zip"), ARCHIVE_ZIP, Some("archive password".into()), at(""), "c".into()).unwrap();
    assert!(!ws.join("a").exists() && !ws.join("b").exists());
    assert!(no_spool(&ws));
}

/// E4.7: a 7z stored in a vault is listed and extracted into the vault with
/// the confined worker; no plaintext lands in a file.
#[test] fn sevenz_inside_a_vault_without_plaintext_files() {
    let _serial = serial();
    let worker = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-pdf-worker");
    if !worker.exists() { eprintln!("paranoic-pdf-worker not built: skipped"); return; }
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-7z-vault-{}", Uuid::new_v4())));
    for d in ["state", "workspace/src/inner"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    let ws = fx.0.join("workspace");
    fs::write(ws.join("src/a.txt"), "seven secret").unwrap();
    fs::write(ws.join("src/inner/b.txt"), "inner seven secret").unwrap();
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), worker.to_str().unwrap().into(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    rt.archive_create(vec![at("src")], ARCHIVE_7Z, None, at(""), "pack.7z".into()).unwrap();
    fs::remove_dir_all(ws.join("src")).unwrap();
    let handle = rt.vault_create(at(""), "Vault".into(), "correct horse battery staple 42".into()).unwrap();
    rt.vault_move_in(handle.clone(), vec![], at("pack.7z"), Uuid::new_v4().to_string()).unwrap();
    let listed = rt.vault_archive_list(handle.clone(), vec![], "pack.7z".into(), ARCHIVE_7Z, None).unwrap();
    assert!(listed.iter().any(|e| e.path == "src/inner/b.txt"));
    assert_eq!(rt.vault_archive_extract(handle.clone(), vec![], "pack.7z".into(), ARCHIVE_7Z, None, vec![], "out".into()).unwrap(), 2);
    let names: Vec<String> = rt.vault_list(handle.clone(), vec!["out".into(), "src".into(), "inner".into()]).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["b.txt"]);
    fn walk(dir: &std::path::Path, out: &mut Vec<u8>) {
        for e in fs::read_dir(dir).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, out) } else { out.extend(fs::read(p).unwrap()) } }
    }
    let mut all = Vec::new(); walk(&ws, &mut all);
    assert!(!all.windows(12).any(|w| w == b"seven secret"), "plaintext found on disk");
    rt.shutdown().unwrap();
}
