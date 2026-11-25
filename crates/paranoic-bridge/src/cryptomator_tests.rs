//! Plan 1.4 part 3: a Cryptomator vault through NativeRuntime, full cycle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf};

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
struct NoSealer;
impl RecordSealer for NoSealer {
    fn established(&self) -> bool { false }
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
}
struct Fixture(PathBuf);
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn walk(p: &std::path::Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(p).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, out) } else { out.push(p) } }
}

/// Cryptomator admits one scrypt derivation per process (KDF_GATE, Busy
/// otherwise); these tests must not derive at the same time.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|e| e.into_inner()) }

#[test] fn cryptomator_full_cycle() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-cm-bridge-{}", Uuid::new_v4())));
    for d in ["state", "workspace/out"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let ws = |p: &str| fx.0.join("workspace").join(p);
    let password = "cryptomator bridge password";
    let report: Vec<u8> = b"quarterly secret report ".iter().copied().cycle().take(100_000).collect();
    fs::write(ws("report.txt"), &report).unwrap();

    let vault = rt.cryptomator_create(at(""), "Vault".into(), password.into()).unwrap();
    assert!(ws("Vault/vault.cryptomator").exists() && ws("Vault/masterkey.cryptomator").exists());
    rt.cryptomator_mkdir(vault.clone(), vec![], "Папка".into()).unwrap();
    rt.cryptomator_add(vault.clone(), vec!["Папка".into()], at("report.txt")).unwrap();
    assert!(matches!(rt.cryptomator_add(vault.clone(), vec!["Папка".into()], at("report.txt")), Err(BridgeError::Conflict)), "replaced");
    let root = rt.cryptomator_list(vault.clone(), vec![]).unwrap();
    assert_eq!(root.entries.iter().map(|e| (e.name.as_str(), e.folder)).collect::<Vec<_>>(), [("Папка", true)]);
    let inner = rt.cryptomator_list(vault.clone(), vec!["Папка".into()]).unwrap();
    assert_eq!(inner.entries.len(), 1);
    assert!(inner.entries[0].size > report.len() as u64, "encrypted size expected");
    // Nothing readable on disk.
    let mut files = Vec::new(); walk(&ws("Vault"), &mut files);
    for f in &files { let b = fs::read(f).unwrap(); assert!(!b.windows(16).any(|w| w == b"quarterly secret"), "{f:?}"); }
    assert!(!files.iter().any(|f| f.to_string_lossy().contains("report") || f.to_string_lossy().contains("Папка")));

    rt.cryptomator_export(vault.clone(), vec!["Папка".into()], "report.txt".into(), at("out")).unwrap();
    assert_eq!(fs::read(ws("out/report.txt")).unwrap(), report);
    assert!(matches!(rt.cryptomator_export(vault.clone(), vec!["Папка".into()], "report.txt".into(), at("out")), Err(BridgeError::Conflict)));
    rt.cryptomator_rename(vault.clone(), vec!["Папка".into()], "report.txt".into(), "renamed.txt".into()).unwrap();
    assert_eq!(rt.cryptomator_list(vault.clone(), vec!["Папка".into()]).unwrap().entries[0].name, "renamed.txt");
    rt.cryptomator_delete(vault.clone(), vec![], "Папка".into()).unwrap();
    assert!(rt.cryptomator_list(vault.clone(), vec![]).unwrap().entries.is_empty());
    assert!(!fs::read_dir(ws("")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")), "spool left");

    // Lock / Quick Lock; reopen; wrong password.
    rt.cryptomator_lock(vault.clone()).unwrap();
    assert!(matches!(rt.cryptomator_list(vault, vec![]), Err(BridgeError::Locked)));
    assert!(matches!(rt.cryptomator_open(at("Vault"), "wrong password".into()), Err(BridgeError::WrongPassword)));
    let again = rt.cryptomator_open(at("Vault"), password.into()).unwrap();
    rt.lock_all_secrets();
    assert!(matches!(rt.cryptomator_list(again, vec![]), Err(BridgeError::Locked)));
    rt.shutdown().unwrap();
}

#[test] fn cryptomator_password_change_survives_interruption() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-cm-pw-{}", Uuid::new_v4())));
    for d in ["state", "workspace"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let ws = |p: &str| fx.0.join("workspace").join(p);
    let backups = || fs::read_dir(ws("Vault")).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".bkup")).count();
    let pending = || fs::read_dir(fx.0.join("state/cryptomator-pending")).map(|d| d.count()).unwrap_or(0);
    let (one, two, three) = ("first password long", "second password long", "third password long");
    let vault = rt.cryptomator_create(at(""), "Vault".into(), one.into()).unwrap();
    rt.cryptomator_mkdir(vault.clone(), vec![], "Папка".into()).unwrap();
    // Wrong current password changes nothing.
    assert!(matches!(rt.cryptomator_change_password(vault.clone(), "wrong password".into(), two.into()), Err(BridgeError::WrongPassword)));
    rt.cryptomator_change_password(vault.clone(), one.into(), two.into()).unwrap();
    assert_eq!((backups(), pending()), (0, 0), "old-password backup or record left");
    assert!(matches!(rt.cryptomator_open(at("Vault"), one.into()), Err(BridgeError::WrongPassword)));
    // Interrupted between steps 2 and 3: the vault has no masterkey file.
    let reader = rt.cryptomator(&vault).unwrap();
    let plan = reader.prepare_password_change_plan(zeroize::Zeroizing::new(three.into()), Uuid::new_v4(), || Ok(())).unwrap();
    rt.save_pending(&plan).unwrap();
    for step in 0..2 {
        let job = plan.job_id(step);
        rt.core.submit_with_id(job, plan.steps()[step].clone()).unwrap();
        crate::features::run_wait(&rt.core, job).unwrap();
    }
    assert!(!ws("Vault/masterkey.cryptomator").exists());
    rt.cryptomator_lock(vault).unwrap();
    // Opening finishes the change, needs the new password, removes the backup.
    let again = rt.cryptomator_open(at("Vault"), three.into()).unwrap();
    assert_eq!(rt.cryptomator_list(again, vec![]).unwrap().entries[0].name, "Папка");
    assert_eq!((backups(), pending()), (0, 0));
    assert!(matches!(rt.cryptomator_open(at("Vault"), two.into()), Err(BridgeError::WrongPassword)));
    rt.shutdown().unwrap();
}

/// E2.8 / Cryptomator on phone storage: a full write cycle in a SAF folder
/// (host behaving like ExternalStorageProvider) — create, folder, files with a
/// short and a long name, rename, delete file and folder, password change.
#[test] fn cryptomator_vault_is_writable_in_a_phone_storage_saf_folder() {
    let _serial = serial();
    let root = std::env::temp_dir().join(format!("paranoic-cm-saf-{}", Uuid::new_v4()));
    for d in ["state", "workspace", "phone/Documents"] { fs::create_dir_all(root.join(d)).unwrap(); }
    let phone = root.join("phone");
    let docs = Arc::new(crate::saf_vault_tests::FsDocs { root: phone.clone(), files: Mutex::new(std::collections::HashMap::new()), next: Mutex::new(0) });
    let rt = NativeRuntime::new(root.join("state").to_str().unwrap().into(), root.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], docs.clone(), Arc::new(NoSealer)).unwrap();
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3ADocuments".into(), host: docs, writable: true };
    let saf_id = saf.id.0.to_string();
    rt.core.register_storage(Arc::new(saf)).unwrap();
    let at_saf = |p: &str| NativeLocation { storage_id: saf_id.clone(), path: p.into() };
    let app = rt.spaces().unwrap()[0].root.clone();
    let at_app = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let ws = root.join("workspace");
    let long = format!("{}.txt", "long-file-name-".repeat(13));
    fs::write(ws.join("short.txt"), "short body").unwrap();
    fs::write(ws.join(&long), "long body").unwrap();

    let (one, two) = ("first password long", "second password long");
    let recent = |rt: &NativeRuntime| rt.diagnostics_recent().iter().rev().take(6).map(|d| format!("{} {}", d.code, d.message)).collect::<Vec<_>>();
    let handle = rt.cryptomator_create(at_saf("Documents"), "Vault".into(), one.into()).unwrap_or_else(|e| panic!("{e:?} {:?} {:?}", recent(&rt), rt.jobs().ok().map(|j| j.iter().map(|x| (x.kind, x.error)).collect::<Vec<_>>())));
    rt.cryptomator_mkdir(handle.clone(), vec![], "Папка".into()).unwrap_or_else(|e| panic!("{e:?} {:?}", recent(&rt)));
    rt.cryptomator_add(handle.clone(), vec![], at_app("short.txt")).unwrap();
    rt.cryptomator_add(handle.clone(), vec!["Папка".into()], at_app(&long)).unwrap();
    rt.cryptomator_rename(handle.clone(), vec![], "short.txt".into(), "renamed.txt".into()).unwrap();
    // Renames that change the shortening class (mapped rename, directory reshape).
    let long2 = format!("{}.txt", "long-renamed-name-".repeat(12));
    let long_dir = "long-folder-name-".repeat(13);
    let err = |e: BridgeError| -> ! { panic!("{e:?} {:?}", rt.diagnostics_recent().iter().rev().take(6).map(|d| format!("{} {}", d.code, d.message)).collect::<Vec<_>>()) };
    rt.cryptomator_rename(handle.clone(), vec![], "renamed.txt".into(), long2.clone()).unwrap_or_else(|e| err(e));
    rt.cryptomator_rename(handle.clone(), vec!["Папка".into()], long.clone(), "short-again.txt".into()).unwrap_or_else(|e| err(e));
    rt.cryptomator_rename(handle.clone(), vec![], "Папка".into(), long_dir.clone()).unwrap_or_else(|e| err(e));
    let listed = |path: Vec<String>| -> Vec<String> { let mut n: Vec<String> = rt.cryptomator_list(handle.clone(), path).unwrap().entries.into_iter().map(|e| e.name).collect(); n.sort(); n };
    let mut expected = vec![long2.clone(), long_dir.clone()]; expected.sort();
    assert_eq!(listed(vec![]), expected);
    assert_eq!(listed(vec![long_dir.clone()]), ["short-again.txt"]);
    rt.cryptomator_rename(handle.clone(), vec![long_dir.clone()], "short-again.txt".into(), long.clone()).unwrap_or_else(|e| err(e));
    rt.cryptomator_rename(handle.clone(), vec![], long_dir.clone(), "Папка".into()).unwrap_or_else(|e| err(e));
    rt.cryptomator_rename(handle.clone(), vec![], long2.clone(), "renamed.txt".into()).unwrap_or_else(|e| err(e));
    rt.cryptomator_mkdir(handle.clone(), vec![], "Пустая".into()).unwrap();
    rt.cryptomator_delete(handle.clone(), vec![], "Пустая".into()).unwrap();
    let names = |path: Vec<String>| -> Vec<String> { let mut n: Vec<String> = rt.cryptomator_list(handle.clone(), path).unwrap().entries.into_iter().map(|e| e.name).collect(); n.sort(); n };
    assert_eq!(names(vec![]), ["renamed.txt", "Папка"]);
    assert_eq!(names(vec!["Папка".into()]), [long.clone()]);
    fs::create_dir(ws.join("out")).unwrap();
    rt.cryptomator_export(handle.clone(), vec!["Папка".into()], long.clone(), at_app("out")).unwrap();
    assert_eq!(fs::read_to_string(ws.join("out").join(&long)).unwrap(), "long body");
    rt.cryptomator_delete(handle.clone(), vec![], "renamed.txt".into()).unwrap();
    rt.cryptomator_change_password(handle.clone(), one.into(), two.into()).unwrap();
    rt.cryptomator_lock(handle).unwrap();
    assert!(matches!(rt.cryptomator_open(at_saf("Documents/Vault"), one.into()), Err(BridgeError::WrongPassword)));
    let handle = rt.cryptomator_open(at_saf("Documents/Vault"), two.into()).unwrap();
    assert_eq!(rt.cryptomator_list(handle, vec!["Папка".into()]).unwrap().entries.len(), 1);
    fn tree(dir: &std::path::Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() { let p = e.unwrap().path(); out.push(p.file_name().unwrap().to_string_lossy().into_owned()); if p.is_dir() { tree(&p, out) } }
    }
    let mut all = Vec::new(); tree(&phone, &mut all);
    assert!(!all.iter().any(|n| n.starts_with(".paranoic-") || n.contains("short") || n.contains("long-file")), "{all:?}");
    rt.shutdown().unwrap();
    let _ = fs::remove_dir_all(&root);
}

/// E4.7 for Cryptomator: random-access stream, text open/save/create with a
/// strong version (same size/time forgery refused), ZIP listed and extracted
/// into the vault; no plaintext on disk; locking the vault ends its streams.
#[test] fn work_inside_a_cryptomator_vault_without_plaintext_files() {
    let _serial = serial();
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-cm-io-{}", Uuid::new_v4())));
    for d in ["state", "workspace"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let ws = fx.0.join("workspace");
    let vault = rt.cryptomator_create(at(""), "Vault".into(), "cryptomator io password".into()).unwrap();
    // Stream.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(ws.join("movie.bin"), &big).unwrap();
    rt.cryptomator_add(vault.clone(), vec![], at("movie.bin")).unwrap();
    fs::remove_file(ws.join("movie.bin")).unwrap();
    let stream = rt.cryptomator_stream_open(vault.clone(), vec![], "movie.bin".into()).unwrap();
    assert_eq!(stream.size, 300_000);
    assert_eq!(rt.vault_stream_read(stream.id.clone(), 32_760, 70_000).unwrap(), big[32_760..102_760]);
    assert_eq!(rt.vault_stream_read(stream.id.clone(), 299_990, 100).unwrap(), big[299_990..]);
    // Text.
    rt.cryptomator_text_create(vault.clone(), vec![], "note.md".into(), "# Тайна\nстрока".into()).unwrap();
    assert!(matches!(rt.cryptomator_text_create(vault.clone(), vec![], "note.md".into(), "x".into()), Err(BridgeError::Conflict)));
    let text = rt.cryptomator_text_open(vault.clone(), vec![], "note.md".into()).unwrap();
    assert_eq!(text.text, "# Тайна\nстрока");
    rt.cryptomator_text_save(vault.clone(), vec![], "note.md".into(), text.version_token.clone(), "# Тайна\nновая строка".into()).unwrap();
    assert!(matches!(rt.cryptomator_text_save(vault.clone(), vec![], "note.md".into(), text.version_token.clone(), "stale".into()),
        Err(BridgeError::Changed)));
    // Same size and time, different content: still refused.
    let opened = rt.cryptomator_text_open(vault.clone(), vec![], "note.md".into()).unwrap();
    let object = || { let mut all = Vec::new(); walk(&ws.join("Vault"), &mut all);
        all.into_iter().filter(|p| p.extension().is_some_and(|e| e == "c9r") && p.is_file() && fs::metadata(p).unwrap().len() == opened.version_bytes)
            .max_by_key(|p| fs::metadata(p).unwrap().modified().unwrap()).unwrap() };
    let stamp = fs::metadata(object()).unwrap().modified().unwrap();
    rt.cryptomator_text_save(vault.clone(), vec![], "note.md".into(), opened.version_token.clone(), "# Тайна\nновая строкА".into()).unwrap();
    fs::File::options().write(true).open(object()).unwrap().set_modified(stamp).unwrap();
    let forged = rt.cryptomator_text_open(vault.clone(), vec![], "note.md".into()).unwrap();
    assert_eq!((forged.version_bytes, forged.version_modified_ms), (opened.version_bytes, opened.version_modified_ms));
    assert!(matches!(rt.cryptomator_text_save(vault.clone(), vec![], "note.md".into(), opened.version_token.clone(), "stale".into()),
        Err(BridgeError::Changed)));
    assert_eq!(rt.cryptomator_text_open(vault.clone(), vec![], "note.md".into()).unwrap().text, "# Тайна\nновая строкА");
    // ZIP inside the vault.
    fs::create_dir_all(ws.join("src/inner")).unwrap();
    fs::write(ws.join("src/a.txt"), "archived secret").unwrap();
    fs::write(ws.join("src/inner/b.txt"), "deeper secret").unwrap();
    rt.archive_create(vec![at("src")], crate::ARCHIVE_ZIP, Some("zip password long".into()), at(""), "pack.zip".into()).unwrap();
    fs::remove_dir_all(ws.join("src")).unwrap();
    rt.cryptomator_add(vault.clone(), vec![], at("pack.zip")).unwrap();
    fs::remove_file(ws.join("pack.zip")).unwrap();
    let listed = rt.cryptomator_archive_list(vault.clone(), vec![], "pack.zip".into(), crate::ARCHIVE_ZIP, None).unwrap();
    assert!(listed.iter().any(|e| e.path == "src/inner/b.txt" && e.encrypted));
    assert!(matches!(rt.cryptomator_archive_extract(vault.clone(), vec![], "pack.zip".into(), crate::ARCHIVE_ZIP, None, vec![], "out".into()),
        Err(BridgeError::PasswordRequired)));
    assert_eq!(rt.cryptomator_archive_extract(vault.clone(), vec![], "pack.zip".into(), crate::ARCHIVE_ZIP, Some("zip password long".into()),
        vec![], "out".into()).unwrap(), 2);
    let inner = rt.cryptomator_text_open(vault.clone(), vec!["out".into(), "src".into(), "inner".into()], "b.txt".into()).unwrap();
    assert_eq!(inner.text, "deeper secret");
    let mut files = Vec::new(); walk(&ws, &mut files);
    let all: Vec<u8> = files.iter().filter(|p| p.is_file()).flat_map(|p| fs::read(p).unwrap()).collect();
    for secret in [&b"archived secret"[..], b"deeper secret", "новая строк".as_bytes()] {
        assert!(!all.windows(secret.len()).any(|w| w == secret), "plaintext found on disk");
    }
    // Office inside both vault kinds, parsed in memory (same view as a plain file).
    fs::write(ws.join("doc.docx"), crate::documents_tests::docx()).unwrap();
    let plain = rt.office_open(at("doc.docx")).unwrap();
    rt.cryptomator_add(vault.clone(), vec![], at("doc.docx")).unwrap();
    let inside = rt.cryptomator_office_open(vault.clone(), vec![], "doc.docx".into()).unwrap();
    let paranoic = rt.vault_create(at(""), "V18".into(), "correct horse battery staple 42".into()).unwrap();
    rt.vault_add(paranoic.clone(), vec![], at("doc.docx")).unwrap();
    let inside18 = rt.vault_office_open(paranoic.clone(), vec![], "doc.docx".into()).unwrap();
    let text = |v: &NativeOfficeView| v.blocks.iter().flat_map(|b| b.runs.iter().map(|r| r.text.clone())).collect::<Vec<_>>();
    assert_eq!((inside.kind, text(&inside)), (plain.kind, text(&plain)));
    assert_eq!((inside18.kind, text(&inside18)), (plain.kind, text(&plain)));
    assert!(rt.cryptomator_office_open(vault.clone(), vec![], "note.md".into()).is_err());
    // A PDF needs the isolated worker: none here, so it is refused (never parsed in-process).
    fs::write(ws.join("doc.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();
    rt.cryptomator_add(vault.clone(), vec![], at("doc.pdf")).unwrap();
    assert!(rt.cryptomator_pdf_page(vault.clone(), vec![], "doc.pdf".into(), 0, 512).is_err());
    rt.vault_lock(paranoic).unwrap();
    // Locking the vault ends its streams.
    rt.cryptomator_lock(vault).unwrap();
    assert!(matches!(rt.vault_stream_read(stream.id, 0, 10), Err(BridgeError::Locked)));
    rt.shutdown().unwrap();
}
