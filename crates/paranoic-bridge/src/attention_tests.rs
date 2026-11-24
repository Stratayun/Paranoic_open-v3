//! Plan 1.3 through NativeRuntime. Failure states are written into the journal
//! exactly as Core leaves them (state, error, active backup entry) and the
//! files are arranged on disk to match, so each case is deterministic.
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
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
}

struct Fixture(PathBuf);
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
impl Fixture {
    fn runtime(&self) -> Arc<NativeRuntime> {
        NativeRuntime::new(self.0.join("state").to_str().unwrap().into(), self.0.join("workspace").to_str().unwrap().into(),
            String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap()
    }
    fn ws(&self, p: &str) -> PathBuf { self.0.join("workspace").join(p) }
    /// Rewrite a finished job as Core leaves an unresolved one.
    fn mark_attention(&self, job: &str, message: &str, active: Option<&str>) {
        let db = fs::read_dir(self.0.join("state")).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .flat_map(|p| if p.is_dir() { fs::read_dir(p).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect() } else { vec![p] })
            .find(|p| p.file_name().is_some_and(|n| n == "operations.sqlite3")).expect("journal");
        let connection = rusqlite::Connection::open(db).unwrap();
        let text: String = connection.query_row("SELECT record FROM jobs WHERE id=?1", [job], |r| r.get(0)).unwrap();
        let mut record: serde_json::Value = serde_json::from_str(&text).unwrap();
        record["state"] = "needs_attention".into();
        record["effects_started"] = true.into();
        record["result"] = serde_json::Value::Null;
        record["error"] = serde_json::json!({ "code": "needs_attention", "message": message });
        if let Some(path) = active {
            let storage = record["operation"]["target"]["storage"].clone();
            record["active_entry"] = serde_json::json!({ "storage": storage, "path": path });
        }
        connection.execute("UPDATE jobs SET state='needs_attention', acknowledged=0, record=?1 WHERE id=?2",
            rusqlite::params![record.to_string(), job]).unwrap();
    }
}
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

#[test] fn attention_jobs_are_listed_explained_and_resolved_by_hand() {
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-attention-{}", Uuid::new_v4())));
    for d in ["state", "workspace"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = fx.runtime();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    for (name, data) in [("new-a.txt", "new A"), ("a.txt", "old A"), ("new-b.txt", "new B"), ("b.txt", "old B")] { fs::write(fx.ws(name), data).unwrap(); }
    let replace_a = wait(&rt, rt.transfer(at("new-a.txt"), at(""), "a.txt".into(), 0, 3, id()).unwrap());
    let replace_b = wait(&rt, rt.transfer(at("new-b.txt"), at(""), "b.txt".into(), 0, 3, id()).unwrap());
    let folder = wait(&rt, rt.create_directory(at(""), "dir".into(), id()).unwrap());
    assert!([&replace_a, &replace_b, &folder].iter().all(|j| j.phase == 3));
    assert!(rt.attention_list().unwrap().is_empty());
    rt.shutdown().unwrap(); drop(rt);

    // A: replacement failed — old version only in the hidden backup, name free.
    let backup_a = format!(".pnc-replaced-{}", replace_a.id);
    fs::rename(fx.ws("a.txt"), fx.ws(&backup_a)).unwrap();
    fs::write(fx.ws(&backup_a), "old A").unwrap();
    fx.mark_attention(&replace_a.id, "replacement failed; previous version retained in hidden backup", Some(&backup_a));
    // B: replaced, but the backup could not be removed — new file in place.
    let backup_b = format!(".pnc-replaced-{}", replace_b.id);
    fs::write(fx.ws(&backup_b), "old B").unwrap();
    fx.mark_attention(&replace_b.id, "file replaced; previous version could not be removed from hidden backup", Some(&backup_b));
    // C: interrupted by a crash while writing.
    fx.mark_attention(&folder.id, "interrupted operation: inspect source, destination and staging artifact", None);

    let rt = fx.runtime();
    let list = rt.attention_list().unwrap();
    let find = |job: &str| list.iter().find(|a| a.job.id == job).cloned().unwrap();
    assert_eq!(list.len(), 3);
    let (a, b, c) = (find(&replace_a.id), find(&replace_b.id), find(&folder.id));
    assert_eq!((a.reason, a.can_restore, a.can_discard), (2, true, false));
    assert_eq!(a.backup.as_ref().unwrap().path, backup_a);
    assert_eq!((b.reason, b.can_restore, b.can_discard), (2, false, true));
    assert_eq!((c.reason, c.can_restore, c.can_discard, c.can_accept), (1, false, false, false));
    assert!(c.detail.starts_with("interrupted operation"));

    // The only remaining version is never deleted; restoring over a present file is refused.
    assert!(matches!(rt.attention_discard(replace_a.id.clone(), id()), Err(BridgeError::Denied)));
    assert!(matches!(rt.attention_restore(replace_b.id.clone(), id()), Err(BridgeError::Conflict)));
    assert_eq!(fs::read(fx.ws(&backup_a)).unwrap(), b"old A");

    // A: restore puts the old version back under its name.
    assert_eq!(wait(&rt, rt.attention_restore(replace_a.id.clone(), id()).unwrap()).phase, 3);
    assert_eq!(fs::read(fx.ws("a.txt")).unwrap(), b"old A");
    assert!(!fx.ws(&backup_a).exists());
    // B: discard deletes only the backup; a backup changed meanwhile is kept.
    fs::write(fx.ws(&backup_b), "old B, edited").unwrap();
    let stale = rt.attention_discard(replace_b.id.clone(), id()).unwrap();
    fs::write(fx.ws(&backup_b), "old B, edited again!").unwrap();
    assert_eq!(wait(&rt, stale).phase, 4, "changed backup: must fail cleanly, not delete or need attention");
    assert!(fx.ws(&backup_b).exists());
    assert_eq!(wait(&rt, rt.attention_discard(replace_b.id.clone(), id()).unwrap()).phase, 3);
    assert!(!fx.ws(&backup_b).exists());
    assert_eq!(fs::read(fx.ws("b.txt")).unwrap(), b"new B");

    // Reviewing closes a job without touching files; only open attention jobs qualify.
    for job in [&replace_a.id, &replace_b.id, &folder.id] { rt.attention_reviewed(job.clone()).unwrap(); }
    assert!(rt.attention_list().unwrap().is_empty());
    assert!(matches!(rt.attention_reviewed(folder.id.clone()), Err(BridgeError::Invalid)), "closed twice");
    let ordinary = wait(&rt, rt.create_directory(at(""), "other".into(), id()).unwrap());
    assert!(matches!(rt.attention_reviewed(ordinary.id), Err(BridgeError::Invalid)));
    assert!(matches!(rt.attention_accept(folder.id.clone()), Err(BridgeError::Invalid)));
    // History keeps the original outcome.
    assert_eq!(rt.job(folder.id.clone()).unwrap().unwrap().phase, 6);
    assert!(fx.ws("dir").is_dir() && fx.ws("a.txt").exists() && fx.ws("b.txt").exists());
    rt.shutdown().unwrap();
}
