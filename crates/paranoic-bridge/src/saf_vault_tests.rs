//! E2.8: a 1.8 vault in a phone-storage SAF folder. The host below behaves
//! like Android's ExternalStorageProvider on a real directory: create and
//! rename never overwrite (they pick "name (1)"), files are written by handle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{collections::HashMap, fs, io::{Read, Seek, SeekFrom, Write}, path::PathBuf};

pub(crate) struct FsDocs { pub(crate) root: PathBuf, pub(crate) files: Mutex<HashMap<u64, fs::File>>, pub(crate) next: Mutex<u64>, pub(crate) before_delete: Mutex<Option<u8>> }
impl FsDocs {
    fn path(&self, rel: &str) -> PathBuf { if rel.is_empty() { self.root.clone() } else { self.root.join(rel) } }
    fn info(meta: &fs::Metadata) -> HostInfo {
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() * 1000);
        HostInfo { error: 0, kind: if meta.is_dir() { 2 } else { 1 }, bytes: if meta.is_dir() { 0 } else { meta.len() }, modified_ms: modified }
    }
    fn missing() -> HostInfo { HostInfo { error: 5, kind: 0, bytes: 0, modified_ms: None } }
    fn unique(&self, parent: &str, name: &str) -> String {
        let mut candidate = name.to_owned();
        let mut n = 1;
        while self.path(&join(parent, &candidate)).exists() { candidate = format!("{name} ({n})"); n += 1; }
        candidate
    }
    fn handle(&self, file: fs::File) -> u64 {
        let mut next = self.next.lock().unwrap(); *next += 1;
        self.files.lock().unwrap().insert(*next, file); *next
    }
}
fn join(parent: &str, name: &str) -> String { if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") } }
fn parent_of(rel: &str) -> &str { rel.rsplit_once('/').map_or("", |(p, _)| p) }
fn created(name: String, document: String) -> HostCreated { HostCreated { error: 0, name, document } }
fn failed(error: u32) -> HostCreated { HostCreated { error, name: String::new(), document: String::new() } }
impl AndroidDocuments for FsDocs {
    fn metadata(&self, _: String, path: String) -> HostInfo { fs::metadata(self.path(&path)).map(|m| Self::info(&m)).unwrap_or_else(|_| Self::missing()) }
    fn list(&self, _: String, path: String, _: u32) -> HostListing {
        let Ok(dir) = fs::read_dir(self.path(&path)) else { return HostListing { error: 5, entries: vec![] } };
        HostListing { error: 0, entries: dir.map(|e| { let e = e.unwrap();
            HostEntry { name: e.file_name().to_string_lossy().into_owned(), info: Self::info(&e.metadata().unwrap()) } }).collect() }
    }
    fn open(&self, _: String, path: String) -> HostOpened {
        match fs::File::open(self.path(&path)) {
            Ok(file) => { let info = Self::info(&file.metadata().unwrap()); HostOpened { error: 0, id: self.handle(file), info } }
            Err(_) => HostOpened { error: 5, id: 0, info: Self::missing() },
        }
    }
    fn read(&self, handle: u64, offset: u64, size: u32) -> HostBytes {
        let mut files = self.files.lock().unwrap();
        let file = files.get_mut(&handle).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        let mut data = vec![0u8; size as usize];
        let n = file.read(&mut data).unwrap(); data.truncate(n);
        HostBytes { error: 0, data }
    }
    fn file_metadata(&self, handle: u64) -> HostInfo { Self::info(&self.files.lock().unwrap()[&handle].metadata().unwrap()) }
    fn close(&self, handle: u64) { self.files.lock().unwrap().remove(&handle); }
    fn writable(&self, _: String) -> bool { true }
    fn create(&self, _: String, parent: String, name: String, directory: bool) -> HostCreated {
        let name = self.unique(&parent, &name);
        let rel = join(&parent, &name);
        let ok = if directory { fs::create_dir(self.path(&rel)).is_ok() } else { fs::File::create_new(self.path(&rel)).is_ok() };
        if ok { created(name, rel) } else { failed(3) }
    }
    fn open_write(&self, _: String, document: String) -> HostOpened {
        let file = fs::OpenOptions::new().write(true).truncate(true).open(self.path(&document)).unwrap();
        HostOpened { error: 0, id: self.handle(file), info: Self::missing() }
    }
    fn write(&self, handle: u64, data: Vec<u8>) -> u32 { self.files.lock().unwrap().get_mut(&handle).unwrap().write_all(&data).map_or(3, |_| 0) }
    fn finish_write(&self, handle: u64) -> u32 { self.files.lock().unwrap().remove(&handle).map_or(3, |f| f.sync_all().map_or(3, |_| 0)) }
    fn rename(&self, _: String, document: String, name: String) -> HostCreated {
        let parent = parent_of(&document).to_owned();
        let name = self.unique(&parent, &name);
        let rel = join(&parent, &name);
        if fs::rename(self.path(&document), self.path(&rel)).is_ok() { created(name, rel) } else { failed(3) }
    }
    fn deletion_supported(&self, _: String) -> bool { true }
    fn delete(&self, _: String, document: String, directory: bool) -> u32 {
        let p = self.path(&document);
        // Deterministic concurrent writer immediately before the typed syscall.
        match self.before_delete.lock().unwrap().take() {
            Some(1) => fs::write(p.join("late-child"), b"keep").unwrap(),
            Some(2) => { fs::remove_file(&p).unwrap(); fs::create_dir(&p).unwrap(); fs::write(p.join("late-child"), b"keep").unwrap(); }
            Some(3) => { fs::remove_dir(&p).unwrap(); fs::write(&p, b"replacement").unwrap(); }
            _ => {}
        }
        let r = if directory { fs::remove_dir(&p) } else { fs::remove_file(&p) };
        r.map_or(3, |_| 0)
    }
    fn resolve(&self, _: String, path: String) -> HostCreated {
        if self.path(&path).exists() { created(path.rsplit('/').next().unwrap_or("").to_owned(), path) } else { failed(5) }
    }
    /// Like FileSystemProvider.moveDocument: the name is kept; an existing
    /// document of that name in the target folder is an error, never replaced.
    fn move_document(&self, _: String, document: String, source_parent: String, target_parent: String) -> HostCreated {
        if parent_of(&document) != source_parent { return failed(3); }
        let name = document.rsplit('/').next().unwrap_or("").to_owned();
        let rel = join(&target_parent, &name);
        if self.path(&rel).exists() { return failed(3); }
        if fs::rename(self.path(&document), self.path(&rel)).is_ok() { created(name, rel) } else { failed(3) }
    }
}
struct NoSealer;
impl RecordSealer for NoSealer {
    fn established(&self) -> bool { false }
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
}
fn tree(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        out.push(p.file_name().unwrap().to_string_lossy().into_owned());
        if p.is_dir() { out.extend(tree(&p)); }
    }
    out
}

#[test] fn paranoic_vault_lives_in_a_phone_storage_saf_folder() {
    let root = std::env::temp_dir().join(format!("paranoic-saf-vault-{}", Uuid::new_v4()));
    let phone = root.join("phone");
    for d in ["state", "workspace", "phone/Documents"] { fs::create_dir_all(root.join(d)).unwrap(); }
    fs::write(phone.join("Documents/unrelated.txt"), "neighbour").unwrap();
    let docs = Arc::new(FsDocs { root: phone.clone(), files: Mutex::new(HashMap::new()), next: Mutex::new(0), before_delete: Mutex::new(None) });
    let rt = NativeRuntime::new(root.join("state").to_str().unwrap().into(), root.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], docs.clone(), Arc::new(NoSealer)).unwrap();
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3ADocuments".into(), host: docs, writable: true };
    let saf_id = saf.id.0.to_string();
    rt.core.register_storage(Arc::new(saf)).unwrap();
    let at_saf = |p: &str| NativeLocation { storage_id: saf_id.clone(), path: p.into() };
    let app = rt.spaces().unwrap()[0].root.clone();
    let at_app = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    fs::write(root.join("workspace/report.txt"), "quarterly numbers").unwrap();

    // Created in a folder that already has other files: a new subfolder.
    let password = "correct horse battery staple 42";
    let handle = rt.vault_create(at_saf("Documents"), "Vault".into(), password.into()).unwrap_or_else(|e| panic!("{e:?} {:?}", rt.diagnostics_recent().iter().map(|d| format!("{} {}", d.code, d.message)).collect::<Vec<_>>()));
    assert_eq!(fs::read_to_string(phone.join("Documents/unrelated.txt")).unwrap(), "neighbour");
    rt.vault_add(handle.clone(), vec![], at_app("report.txt")).unwrap();
    rt.vault_mkdir(handle.clone(), vec![], "Folder".into()).unwrap();
    rt.vault_mkdir(handle.clone(), vec![], "Empty".into()).unwrap();
    rt.vault_delete(handle.clone(), vec![], "Empty".into(), true).unwrap();
    let names: Vec<String> = rt.vault_list(handle.clone(), vec![]).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // Between folders (host moveDocument, then a rename of the object).
    fs::write(root.join("workspace/note.txt"), "moved note").unwrap();
    rt.vault_add(handle.clone(), vec![], at_app("note.txt")).unwrap();
    rt.vault_move(handle.clone(), vec![], "note.txt".into(), false, vec!["Folder".into()]).unwrap();
    let inside: Vec<String> = rt.vault_list(handle.clone(), vec!["Folder".into()]).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(inside, ["note.txt"]);
    assert_eq!(rt.vault_list(handle.clone(), vec![]).unwrap().entries.len(), 2, "the moved note is still at the top");
    fs::create_dir(root.join("workspace/out")).unwrap();
    rt.vault_export(handle.clone(), vec![], "report.txt".into(), at_app("out")).unwrap();
    assert_eq!(fs::read_to_string(root.join("workspace/out/report.txt")).unwrap(), "quarterly numbers");
    // Replacement paths: the header (password, recovery key) and a file's new version.
    rt.vault_change_password(handle.clone(), password.into(), "another long passphrase 99 river".into()).unwrap();
    let recovery = rt.vault_set_recovery(handle.clone(), "another long passphrase 99 river".into()).unwrap();
    fs::write(root.join("workspace/report.txt"), "quarterly numbers, revised").unwrap();
    rt.vault_replace(handle.clone(), vec![], "report.txt".into(), at_app("report.txt")).unwrap();
    rt.vault_rename(handle.clone(), vec![], "report.txt".into(), "renamed.txt".into(), false).unwrap();
    rt.vault_lock(handle).unwrap();
    assert!(matches!(rt.vault_open(at_saf("Documents/Vault"), password.into()), Err(BridgeError::WrongPassword)));
    let handle = rt.vault_open(at_saf("Documents/Vault"), "another long passphrase 99 river".into()).unwrap();
    rt.vault_export(handle.clone(), vec![], "renamed.txt".into(), at_app("")).unwrap();
    assert_eq!(fs::read_to_string(root.join("workspace/renamed.txt")).unwrap(), "quarterly numbers, revised");
    rt.vault_lock(handle).unwrap();
    rt.vault_open_recovery(at_saf("Documents/Vault"), recovery).unwrap();
    // Nothing plaintext and no temporary objects left in the phone folder.
    let names = tree(&phone);
    assert!(!names.iter().any(|n| n.starts_with(".paranoic-") || n.starts_with(".pnc-") || n.contains("report")), "{names:?}");
    let raw: Vec<u8> = names.iter().filter_map(|_| None::<u8>).collect();
    let _ = raw;
    rt.shutdown().unwrap();
    let _ = fs::remove_dir_all(&root);
}

/// E2.8: moving between folders with a new name — moved, renamed, nothing
/// replaced; a taken name in the target folder is refused before anything moves.
#[test] fn saf_move_between_folders_renames_and_never_replaces() {
    use paranoic_core::Storage;
    let root = std::env::temp_dir().join(format!("paranoic-saf-move-{}", Uuid::new_v4()));
    for d in ["A", "B"] { fs::create_dir_all(root.join(d)).unwrap(); }
    fs::write(root.join("A/x"), "payload").unwrap();
    fs::write(root.join("B/taken"), "keep me").unwrap();
    let docs = Arc::new(FsDocs { root: root.clone(), files: Mutex::new(HashMap::new()), next: Mutex::new(0), before_delete: Mutex::new(None) });
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3A".into(), host: docs, writable: true };
    let p = |s: &str| ResourcePath::parse(s).unwrap();
    assert_eq!(saf.move_no_replace(&p("A/x"), &p("B/taken")).err().unwrap().code, paranoic_core::ErrorCode::AlreadyExists);
    assert_eq!(fs::read_to_string(root.join("B/taken")).unwrap(), "keep me");
    assert!(root.join("A/x").exists());
    saf.move_no_replace(&p("A/x"), &p("B/y")).unwrap();
    assert!(!root.join("A/x").exists() && !root.join("B/x").exists());
    assert_eq!(fs::read_to_string(root.join("B/y")).unwrap(), "payload");
    let _ = fs::remove_dir_all(&root);
}

/// Empty SAF folders can be deleted (vault folders); a folder with anything in
/// it — even a hidden object — is refused and left intact (provider delete is recursive).
#[test] fn saf_deletes_only_empty_folders() {
    use paranoic_core::Storage;
    let root = std::env::temp_dir().join(format!("paranoic-saf-rmdir-{}", Uuid::new_v4()));
    for d in ["empty", "full"] { fs::create_dir_all(root.join(d)).unwrap(); }
    fs::write(root.join("full/.hidden"), "keep").unwrap();
    let docs = Arc::new(FsDocs { root: root.clone(), files: Mutex::new(HashMap::new()), next: Mutex::new(0), before_delete: Mutex::new(None) });
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3A".into(), host: docs, writable: true };
    let p = |s: &str| ResourcePath::parse(s).unwrap();
    assert!(saf.delete_empty_directory(&p("full")).is_err());
    assert_eq!(fs::read_to_string(root.join("full/.hidden")).unwrap(), "keep");
    saf.delete_empty_directory(&p("empty")).unwrap();
    assert!(!root.join("empty").exists());
    let _ = fs::remove_dir_all(&root);
}

/// E2.8: Cryptomator renames that change the shortening class on SAF —
/// success, refusal on a taken name, and recovery of an interrupted rename
/// from the hidden staging in both directions (complete / restore).
#[test] fn saf_mapped_rename_and_reshape_recover_from_staging() {
    use paranoic_core::{DirectoryReshape, ErrorCode, JobId, MappedEntryObservation as O, MappedRecoveryAction as A, Storage};
    let root = std::env::temp_dir().join(format!("paranoic-saf-mapped-{}", Uuid::new_v4()));
    let d = root.join("d");
    fs::create_dir_all(d.join("W.c9s")).unwrap();
    fs::write(d.join("W.c9s/name.c9s"), "old-long").unwrap();
    fs::write(d.join("W.c9s/contents.c9r"), "cipher").unwrap();
    fs::create_dir_all(d.join("taken.c9s")).unwrap();
    let docs = Arc::new(FsDocs { root: root.clone(), files: Mutex::new(HashMap::new()), next: Mutex::new(0), before_delete: Mutex::new(None) });
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3A".into(), host: docs, writable: true };
    let p = |s: &str| ResourcePath::parse(s).unwrap();
    let (old, new) = (b"old-long".as_slice(), b"new-long".as_slice());
    let mut ok = || Ok(());

    // Taken target: refused, nothing moved.
    assert_eq!(saf.rename_mapped(&p("d/W.c9s"), &p("d/taken.c9s"), JobId(Uuid::new_v4()), old, new).unwrap_err().code, ErrorCode::AlreadyExists);
    assert!(d.join("W.c9s/name.c9s").exists());
    // Success: mapping replaced, content kept, no staging left.
    saf.rename_mapped(&p("d/W.c9s"), &p("d/N.c9s"), JobId(Uuid::new_v4()), old, new).unwrap();
    assert_eq!(fs::read(d.join("N.c9s/name.c9s")).unwrap(), new);
    assert_eq!(fs::read_to_string(d.join("N.c9s/contents.c9r")).unwrap(), "cipher");
    assert!(!tree(&root).iter().any(|n| n.contains(".paranoic-")), "{:?}", tree(&root));

    // Interrupted after the mapping swap: staging holds entry (new) + evidence (old).
    let job = JobId(Uuid::new_v4());
    let staging = d.join(format!(".paranoic-{}", job.0));
    fs::create_dir_all(&staging).unwrap();
    fs::rename(d.join("N.c9s"), staging.join("entry")).unwrap();
    fs::write(staging.join("old-mapping"), old).unwrap();
    let seen = saf.inspect_mapped_rename(&p("d/N.c9s"), &p("d/Z.c9s"), job, new, old, &mut ok).unwrap();
    assert_eq!((seen.source, seen.target, seen.staged_entry, seen.staging_present), (O::Missing, O::Missing, O::OldMapping, true));
    // Restore the source: the staged mapping is put back, the replaced one stays as evidence.
    saf.recover_mapped(&p("d/N.c9s"), &p("d/Z.c9s"), job, JobId(Uuid::new_v4()), new, old, A::RestoreSource).unwrap();
    assert_eq!(fs::read(d.join("N.c9s/name.c9s")).unwrap(), new);
    assert_eq!(fs::read_to_string(d.join("N.c9s/contents.c9r")).unwrap(), "cipher");
    // Recovery refuses while a public entry exists.
    fs::create_dir_all(&staging).unwrap();
    assert!(saf.recover_mapped(&p("d/N.c9s"), &p("d/Z.c9s"), job, JobId(Uuid::new_v4()), new, old, A::Complete).is_err());
    // Complete a rename interrupted before the swap (entry still has the source mapping).
    fs::rename(d.join("N.c9s"), staging.join("entry")).unwrap();
    saf.recover_mapped(&p("d/N.c9s"), &p("d/Z.c9s"), job, JobId(Uuid::new_v4()), new, old, A::Complete).unwrap();
    assert_eq!(fs::read(d.join("Z.c9s/name.c9s")).unwrap(), old);
    assert_eq!(fs::read_to_string(d.join("Z.c9s/contents.c9r")).unwrap(), "cipher");

    // Directory reshape: short wrapper (dir.c9r only) <-> long (dir.c9r + name.c9s).
    let link = Uuid::new_v4().to_string();
    fs::create_dir_all(d.join("S.c9r")).unwrap();
    fs::write(d.join("S.c9r/dir.c9r"), &link).unwrap();
    let layout = DirectoryReshape { link: link.clone().into_bytes(), old_mapping: None, new_mapping: Some(b"dir-long".to_vec()) };
    saf.reshape_directory(&p("d/S.c9r"), &p("d/L.c9s"), JobId(Uuid::new_v4()), &layout).unwrap();
    assert_eq!(fs::read(d.join("L.c9s/name.c9s")).unwrap(), b"dir-long");
    assert_eq!(fs::read_to_string(d.join("L.c9s/dir.c9r")).unwrap(), link);
    assert!(!d.join("S.c9r").exists());
    let back = DirectoryReshape { link: link.clone().into_bytes(), old_mapping: Some(b"dir-long".to_vec()), new_mapping: None };
    // Interrupted after the old mapping was moved out: entry is already short.
    let job = JobId(Uuid::new_v4());
    let staging = d.join(format!(".paranoic-{}", job.0));
    fs::create_dir_all(&staging).unwrap();
    fs::rename(d.join("L.c9s"), staging.join("entry")).unwrap();
    fs::rename(staging.join("entry/name.c9s"), staging.join("old-mapping")).unwrap();
    let seen = saf.inspect_reshape_directory(&p("d/L.c9s"), &p("d/S.c9r"), job, &back, &mut ok).unwrap();
    assert_eq!((seen.staged_entry, seen.staging_present), (O::NewMapping, true));
    saf.recover_reshape_directory(&p("d/L.c9s"), &p("d/S.c9r"), job, JobId(Uuid::new_v4()), &back, A::RestoreSource).unwrap();
    assert_eq!(fs::read(d.join("L.c9s/name.c9s")).unwrap(), b"dir-long");
    assert!(staging.join("old-mapping").exists(), "evidence is never deleted by recovery");
    let _ = fs::remove_dir_all(&root);
}


#[test] fn saf_deletion_preserves_late_children_and_type_substitutions() {
    use paranoic_core::Storage;
    for race in 1..=3 {
        let root = std::env::temp_dir().join(format!("paranoic-saf-delete-race-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let target = root.join("target");
        if race == 2 { fs::write(&target, b"old").unwrap(); } else { fs::create_dir(&target).unwrap(); }
        let docs = Arc::new(FsDocs { root: root.clone(), files: Mutex::new(HashMap::new()),
            next: Mutex::new(0), before_delete: Mutex::new(Some(race)) });
        let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
            tree: "content://com.android.externalstorage.documents/tree/primary%3A".into(), host: docs, writable: true };
        let path = ResourcePath::parse("target").unwrap();
        let result = if race == 2 { saf.delete_file(&path) } else { saf.delete_empty_directory(&path) };
        assert!(result.is_err(), "race {race} must preserve the replacement");
        if race == 3 { assert_eq!(fs::read(&target).unwrap(), b"replacement"); }
        else { assert_eq!(fs::read(target.join("late-child")).unwrap(), b"keep"); }
        assert!(saf.delete_empty_directory(&ResourcePath::root()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test] fn saf_deletion_preserves_nonempty_staging_bundles_on_abort() {
    use paranoic_core::Storage;
    let root = std::env::temp_dir().join(format!("paranoic-saf-bundle-cleanup-{}", Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let docs = Arc::new(FsDocs { root: root.clone(), files: Mutex::new(HashMap::new()),
        next: Mutex::new(0), before_delete: Mutex::new(None) });
    let saf = android::SafStorage { id: StorageId(Uuid::new_v4()),
        tree: "content://com.android.externalstorage.documents/tree/primary%3A".into(), host: docs, writable: true };
    let stage = saf.stage_bundle(&ResourcePath::parse("target").unwrap(), JobId(Uuid::new_v4()),
        "content", "mapping", b"mapping").unwrap();
    let hidden = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    fs::write(hidden.join("unexpected"), b"keep").unwrap();
    assert_eq!(stage.abort().err().unwrap().code, paranoic_core::ErrorCode::NeedsAttention);
    assert_eq!(fs::read(hidden.join("unexpected")).unwrap(), b"keep");
    assert_eq!(fs::read(hidden.join("mapping")).unwrap(), b"mapping");
    fs::remove_dir_all(root).unwrap();
}
