//! Plan 2.1: ZIP (AES) and 7z through NativeRuntime — create, list, extract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf};

use crate::test_support::NoSealer;
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
        String::new(), String::new(), vec![], Arc::new(NoSealer)).unwrap();
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
        String::new(), worker.to_str().unwrap().into(), vec![], Arc::new(NoSealer)).unwrap();
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
    let ext = rt.attach(NativeGrant { id: Uuid::new_v4().to_string(), reference: fx.0.join("external").to_str().unwrap().into() }).unwrap();
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
        String::new(), String::new(), vec![], Arc::new(NoSealer)).unwrap();
    assert!(!ws.join(".pnc-spool-c0ffee00c0ffee00c0ffee00c0ffee00").exists(), "crashed spool folder kept");
}


#[test] fn archive_password_outcomes_reach_the_host() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-archive-pw-{}", Uuid::new_v4())));
    for d in ["state", "workspace/src"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    let ws = fx.0.join("workspace");
    fs::write(ws.join("src/a.txt"), "secret").unwrap();
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoSealer)).unwrap();
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
        String::new(), worker.to_str().unwrap().into(), vec![], Arc::new(NoSealer)).unwrap();
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
