//! Plan 1.1/1.2: transfer (copy/move with conflict choice), rename and delete
//! through NativeRuntime, as the file manager calls them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf, time::{Duration, Instant}};

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
fn id() -> String { Uuid::new_v4().to_string() }
fn wait(rt: &NativeRuntime, job: NativeJob) -> NativeJob {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let j = rt.job(job.id.clone()).unwrap().unwrap();
        if j.phase >= 3 { return j; }
        assert!(Instant::now() < until, "job timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn at(space: &NativeSpace, path: &str) -> NativeLocation { NativeLocation { storage_id: space.root.storage_id.clone(), path: path.into() } }

#[test] fn transfer_rename_delete() {
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-files-{}", Uuid::new_v4())));
    for d in ["state", "workspace", "external"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].clone();
    let ext = rt.attach(NativeGrant { id: id(), reference: fx.0.join("external").to_str().unwrap().into(), android_tree: false }).unwrap();
    let ws = |p: &str| fx.0.join("workspace").join(p);
    let ex = |p: &str| fx.0.join("external").join(p);
    fs::write(ws("a.txt"), b"first").unwrap();
    fs::write(ws("b.txt"), b"second").unwrap();
    fs::create_dir_all(ws("dir/sub")).unwrap();
    fs::write(ws("dir/sub/n.txt"), b"nested").unwrap();
    fs::create_dir(ws("into")).unwrap();

    // Copy: refuse / skip / keep both / replace.
    let copy = |src: &str, name: &str, conflict: u8| wait(&rt, rt.transfer(at(&app, src), at(&app, ""), name.into(), 0, conflict, id()).unwrap());
    let j = copy("a.txt", "b.txt", 0); assert_eq!((j.phase, j.error), (4, Some(2)), "refuse must fail with AlreadyExists");
    assert_eq!(fs::read(ws("b.txt")).unwrap(), b"second");
    let j = copy("a.txt", "b.txt", 1);
    assert_eq!((j.phase, j.outcome), (3, 2), "skip must report Skipped"); assert!(j.result_target.is_none());
    assert_eq!(fs::read(ws("b.txt")).unwrap(), b"second");
    let j = copy("a.txt", "b.txt", 2);
    assert_eq!((j.phase, j.outcome), (3, 1));
    assert_eq!(j.result_target.map(|t| t.path).as_deref(), Some("b (1).txt"), "keep both must report the chosen name");
    assert_eq!(fs::read(ws("b (1).txt")).unwrap(), b"first");
    let j = copy("a.txt", "b.txt", 3);
    assert_eq!((j.phase, j.outcome), (3, 1)); assert!(j.result_target.is_none());
    assert_eq!(fs::read(ws("b.txt")).unwrap(), b"first");
    assert_eq!(rt.job(j.id).unwrap().unwrap().outcome, 1);
    assert!(matches!(rt.transfer(at(&app, "a.txt"), at(&app, ""), "x".into(), 0, 9, id()), Err(BridgeError::Invalid)));

    // Move within the same storage; a move never replaces and never goes into itself.
    assert_eq!(wait(&rt, rt.transfer(at(&app, "a.txt"), at(&app, "into"), "a.txt".into(), 1, 0, id()).unwrap()).phase, 3);
    assert!(!ws("a.txt").exists() && fs::read(ws("into/a.txt")).unwrap() == b"first");
    assert!(matches!(rt.transfer(at(&app, "b.txt"), at(&app, ""), "b (1).txt".into(), 1, 3, id()), Err(BridgeError::Invalid)));
    let j = wait(&rt, rt.transfer(at(&app, "b.txt"), at(&app, ""), "b (1).txt".into(), 1, 0, id()).unwrap());
    assert_eq!((j.phase, j.error), (4, Some(2)));
    assert_eq!(fs::read(ws("b.txt")).unwrap(), b"first");
    assert!(matches!(rt.transfer(at(&app, "dir"), at(&app, "dir/sub"), "dir".into(), 1, 0, id()), Err(BridgeError::Invalid)));

    // Move across storages: file and folder tree arrive, sources removed.
    assert_eq!(wait(&rt, rt.transfer(at(&app, "into/a.txt"), at(&ext, ""), "a.txt".into(), 1, 0, id()).unwrap()).phase, 3);
    assert!(!ws("into/a.txt").exists() && fs::read(ex("a.txt")).unwrap() == b"first");
    assert_eq!(wait(&rt, rt.transfer(at(&app, "dir"), at(&ext, ""), "dir".into(), 1, 0, id()).unwrap()).phase, 3);
    assert!(!ws("dir").exists() && fs::read(ex("dir/sub/n.txt")).unwrap() == b"nested");

    // Rename: in place, shown as a rename, never replaces.
    let j = wait(&rt, rt.rename(at(&app, "b.txt"), "c.txt".into(), id()).unwrap());
    assert_eq!((j.phase, j.kind), (3, 8));
    assert!(!ws("b.txt").exists() && ws("c.txt").exists());
    assert_eq!(wait(&rt, rt.rename(at(&app, "c.txt"), "b (1).txt".into(), id()).unwrap()).phase, 4);
    assert!(ws("c.txt").exists());
    assert!(rt.rename(at(&app, ""), "x".into(), id()).is_err(), "space root renamed");

    // Delete: file, empty folder, non-empty folder only when recursive, never a root.
    assert_eq!(wait(&rt, rt.delete(at(&app, "c.txt"), false, id()).unwrap()).phase, 3);
    assert!(!ws("c.txt").exists());
    assert_eq!(wait(&rt, rt.delete(at(&ext, "dir"), false, id()).unwrap()).phase, 4);
    assert!(ex("dir/sub/n.txt").exists(), "non-recursive delete removed content");
    assert_eq!(wait(&rt, rt.delete(at(&ext, "dir"), true, id()).unwrap()).phase, 3);
    assert!(!ex("dir").exists());
    assert_eq!(wait(&rt, rt.delete(at(&app, "into"), false, id()).unwrap()).phase, 3);
    assert!(matches!(rt.delete(at(&app, ""), true, id()), Err(BridgeError::Invalid)));
    assert!(ws("b (1).txt").exists() && ex("a.txt").exists(), "unrelated files touched");
    // The same request ID never runs twice.
    let rid = id();
    let first = rt.delete(at(&app, "b (1).txt"), false, rid.clone()).unwrap();
    let again = rt.delete(at(&app, "b (1).txt"), false, rid).unwrap_or_else(|_| first.clone());
    assert_eq!(first.id, again.id);
    rt.shutdown().unwrap();
}

/// Trash: delete moves into .Trash-Paranoic, restore puts it back without
/// replacing, purge erases only that entry.
#[test] fn trash_put_list_restore_purge() {
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-trash-{}", Uuid::new_v4())));
    for d in ["state", "workspace", "external"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let ext = rt.attach(NativeGrant { id: id(), reference: fx.0.join("external").to_str().unwrap().into(), android_tree: false }).unwrap();
    let ex = |p: &str| fx.0.join("external").join(p);
    fs::create_dir_all(ex("docs/deep")).unwrap();
    fs::write(ex("docs/deep/a.txt"), b"keep me").unwrap();
    fs::write(ex("b.txt"), b"bee").unwrap();
    assert!(rt.trash_supported(ext.root.storage_id.clone()));
    rt.trash_put(at(&ext, "docs/deep/a.txt")).unwrap();
    rt.trash_put(at(&ext, "b.txt")).unwrap();
    assert!(!ex("docs/deep/a.txt").exists() && !ex("b.txt").exists());
    let items = rt.trash_list(ext.root.storage_id.clone()).unwrap();
    assert_eq!(items.len(), 2);
    let a = items.iter().find(|i| i.name == "a.txt").unwrap().clone();
    assert_eq!(a.original_folder, "docs/deep");
    assert!(rt.trash_put(at(&ext, ".Trash-Paranoic")).is_err(), "the trash itself cannot be trashed");
    // Restore refuses to replace a new file of the same name.
    fs::write(ex("docs/deep/a.txt"), b"newer").unwrap();
    assert!(rt.trash_restore(ext.root.storage_id.clone(), a.bucket.clone()).is_err());
    assert_eq!(fs::read(ex("docs/deep/a.txt")).unwrap(), b"newer");
    fs::remove_file(ex("docs/deep/a.txt")).unwrap();
    rt.trash_restore(ext.root.storage_id.clone(), a.bucket.clone()).unwrap();
    assert_eq!(fs::read(ex("docs/deep/a.txt")).unwrap(), b"keep me");
    assert_eq!(rt.trash_empty(ext.root.storage_id.clone()).unwrap(), 1);
    assert!(rt.trash_list(ext.root.storage_id.clone()).unwrap().is_empty());
    assert!(ex("docs/deep/a.txt").exists(), "purge touched a restored file");
}
