//! Synthetic host contracts; no device, PDFium, real grants or network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use paranoic_core::{ErrorCode, Storage};
use std::{fs, io::Write};

#[derive(Default)]
struct HostState {
    names: Vec<String>,
    list_error: u32,
    target_exists: bool,
    chosen_name: Option<String>,
    finish_error: u32,
    created: Vec<String>,
    deleted: Vec<String>,
    bytes: Vec<u8>,
    closed: usize,
}
#[derive(Default)]
struct Host(Mutex<HostState>);
/// Unsealed journal for synthetic contracts.
struct NoSealer;
impl RecordSealer for NoSealer {
    fn established(&self) -> bool { false }
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: Vec::new() } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: Vec::new() } }
}
fn info() -> HostInfo { HostInfo { error: 0, kind: 1, bytes: 4, modified_ms: Some(1000) } }
impl AndroidDocuments for Host {
    fn metadata(&self, _: String, path: String) -> HostInfo {
        let s = self.0.lock().unwrap();
        if path == "target" && !s.target_exists { HostInfo { error: 5, ..info() } } else { info() }
    }
    fn list(&self, _: String, _: String, _: u32) -> HostListing {
        let s = self.0.lock().unwrap();
        HostListing { error: s.list_error, entries: s.names.iter().map(|name| HostEntry { name: name.clone(), info: info() }).collect() }
    }
    fn open(&self, _: String, _: String) -> HostOpened { HostOpened { error: 0, id: 1, info: info() } }
    fn read(&self, _: u64, _: u64, _: u32) -> HostBytes { HostBytes { error: 0, data: self.0.lock().unwrap().bytes.clone() } }
    fn file_metadata(&self, _: u64) -> HostInfo { info() }
    fn close(&self, _: u64) { self.0.lock().unwrap().closed += 1; }
    fn writable(&self, _: String) -> bool { true }
    fn create(&self, _: String, _: String, name: String, _: bool) -> HostCreated {
        self.0.lock().unwrap().created.push(name.clone()); HostCreated { error: 0, document: name.clone(), name }
    }
    fn open_write(&self, _: String, _: String) -> HostOpened { HostOpened { error: 0, id: 2, info: info() } }
    fn write(&self, _: u64, data: Vec<u8>) -> u32 { self.0.lock().unwrap().bytes.extend(data); 0 }
    fn finish_write(&self, _: u64) -> u32 { self.0.lock().unwrap().finish_error }
    fn rename(&self, _: String, _: String, name: String) -> HostCreated {
        let actual = self.0.lock().unwrap().chosen_name.clone().unwrap_or(name);
        HostCreated { error: 0, document: actual.clone(), name: actual }
    }
    fn delete(&self, _: String, document: String, _: bool) -> u32 { self.0.lock().unwrap().deleted.push(document); 0 }
    fn resolve(&self, _: String, path: String) -> HostCreated { HostCreated { error: 0, document: path.clone(), name: path } }
    fn move_document(&self, _: String, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
}
fn saf(host: &Arc<Host>, writable: bool) -> android::SafStorage {
    android::SafStorage { id: StorageId(Uuid::new_v4()), tree: "content://fixture.invalid/tree/test".into(), host: host.clone(), writable }
}
fn path(name: &str) -> ResourcePath { ResourcePath::parse(name.to_owned()).unwrap() }

#[test]
fn saf_deletion_without_atomic_host_support_never_reaches_a_mutation() {
    let host = Arc::new(Host::default());
    let storage = saf(&host, true);
    assert!(!storage.capabilities().delete);
    assert_eq!(storage.delete_file(&path("file")).err().unwrap().code, ErrorCode::Unsupported);
    assert_eq!(storage.delete_empty_directory(&path("folder")).err().unwrap().code, ErrorCode::Unsupported);
    assert!(host.0.lock().unwrap().deleted.is_empty());
}

#[test]
fn saf_display_counts_every_ambiguous_or_nonportable_entry_but_strict_operations_fail() {
    let host = Arc::new(Host::default());
    host.0.lock().unwrap().names = ["ok.txt", "bad:name", "question?", "trailing.", "same", "same"]
        .into_iter().map(str::to_owned).collect();
    let storage = saf(&host, false);
    let listing = storage.list_for_display(&ResourcePath::root(), 16).unwrap();
    assert_eq!(listing.skipped, 5);
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].location.path.as_str(), "ok.txt");
    assert_eq!(storage.list(&ResourcePath::root(), 16).err().unwrap().code, ErrorCode::Unsupported);
}

#[test]
fn saf_provider_failure_and_oversized_listing_are_not_empty_success() {
    let host = Arc::new(Host::default());
    let storage = saf(&host, false);
    host.0.lock().unwrap().list_error = 1;
    assert_eq!(storage.list_for_display(&ResourcePath::root(), 1).err().unwrap().code, ErrorCode::PermissionDenied);
    let mut s = host.0.lock().unwrap(); s.list_error = 0; s.names = vec!["a".into(), "b".into()]; drop(s);
    assert_eq!(storage.list_for_display(&ResourcePath::root(), 1).err().unwrap().code, ErrorCode::LimitExceeded);
}

#[test]
fn saf_read_only_never_creates_and_existing_target_is_never_replaced() {
    let host = Arc::new(Host::default());
    assert_eq!(saf(&host, false).stage(&path("target"), JobId(Uuid::new_v4())).err().unwrap().code, ErrorCode::PermissionDenied);
    assert!(host.0.lock().unwrap().created.is_empty());
    // E2.8: staging is allowed while the name is taken (Core stages a
    // replacement before moving the old file aside); publication never
    // replaces: the provider picks another name, our document is removed.
    host.0.lock().unwrap().target_exists = true;
    host.0.lock().unwrap().chosen_name = Some("target (1)".into());
    let staged = saf(&host, true).stage(&path("target"), JobId(Uuid::new_v4())).unwrap();
    assert_eq!(staged.publish().err().unwrap().code, ErrorCode::AlreadyExists);
    assert_eq!(host.0.lock().unwrap().deleted, ["target (1)"], "only our own renamed document is removed");
}

#[test]
fn saf_provider_chosen_conflict_name_cleans_only_our_new_document() {
    let host = Arc::new(Host::default());
    host.0.lock().unwrap().chosen_name = Some("target (1)".into());
    let storage = saf(&host, true);
    let mut stage = storage.stage(&path("target"), JobId(Uuid::new_v4())).unwrap();
    stage.write_all(b"new bytes").unwrap();
    assert_eq!(stage.publish().unwrap_err().code, ErrorCode::AlreadyExists);
    let state = host.0.lock().unwrap();
    assert_eq!(state.deleted, vec!["target (1)"]);
    assert_eq!(state.bytes, b"new bytes");
    assert_eq!(state.created.len(), 1);
    assert!(state.created[0].starts_with(".paranoic-"));
}

#[test]
fn saf_finish_failure_discards_own_temporary_document() {
    let host = Arc::new(Host::default());
    host.0.lock().unwrap().finish_error = 3;
    let storage = saf(&host, true);
    let stage = storage.stage(&path("target"), JobId(Uuid::new_v4())).unwrap();
    assert!(stage.publish().is_err());
    let state = host.0.lock().unwrap();
    assert_eq!(state.deleted, state.created);
    assert!(!state.deleted.iter().any(|name| name == "target"));
}

#[test]
fn saf_oversized_host_read_is_rejected_and_handle_is_closed() {
    use std::io::Read;
    let host = Arc::new(Host::default());
    host.0.lock().unwrap().bytes = vec![1; 9];
    let storage = saf(&host, false);
    let mut reader = storage.open_read(&path("file")).unwrap();
    assert!(reader.read(&mut [0u8; 8]).is_err());
    drop(reader);
    assert_eq!(host.0.lock().unwrap().closed, 1);
}

/// Owns only a freshly created directory; never accepts a caller-supplied path.
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("paranoic-test-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        for name in ["state", "workspace", "external"] { fs::create_dir(dir.join(name)).unwrap(); }
        Self(dir)
    }
    fn runtime(&self) -> Arc<NativeRuntime> {
        NativeRuntime::new(self.0.join("state").to_str().unwrap().into(), self.0.join("workspace").to_str().unwrap().into(),
            String::new(), String::new(), vec![], Arc::new(Host::default()), Arc::new(NoSealer)).unwrap()
    }
    fn grant(&self, name: &str) -> NativeGrant {
        NativeGrant { id: Uuid::new_v4().to_string(), reference: self.0.join(name).to_str().unwrap().into(), android_tree: false }
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn native_reader_enforces_boundaries_change_detection_and_close() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("workspace/file"), b"0123456789").unwrap();
    let runtime = fixture.runtime();
    let root = runtime.spaces().unwrap().remove(0).root;
    let reader = runtime.open_stream(NativeLocation { path: "file".into(), ..root }).unwrap();
    assert_eq!(reader.length(), 10);
    assert_eq!(reader.read_at(8, 100).unwrap(), b"89");
    assert!(reader.read_at(u64::MAX, 1).unwrap().is_empty());
    assert!(matches!(reader.read_at(0, 1024 * 1024 + 1), Err(BridgeError::Limit)));
    // Unix permits mutation under an open read handle; Windows may deny the writer.
    #[cfg(unix)]
    {
        fs::write(fixture.0.join("workspace/file"), b"changed length").unwrap();
        assert!(matches!(reader.read_at(0, 1), Err(BridgeError::Changed)));
    }
    reader.close(); reader.close();
    assert!(matches!(reader.read_at(0, 1), Err(BridgeError::Denied)));
    runtime.shutdown().unwrap();
}

#[test]
fn native_runtime_rejects_internal_grants_invalid_ids_and_traversal() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    assert!(matches!(runtime.attach(fixture.grant("state")), Err(BridgeError::Denied)));
    assert!(matches!(runtime.attach(fixture.grant("workspace")), Err(BridgeError::Denied)));
    let mut grant = fixture.grant("external"); grant.id = "not-a-uuid".into();
    assert!(matches!(runtime.attach(grant), Err(BridgeError::Invalid)));
    let root = runtime.spaces().unwrap().remove(0).root;
    assert!(matches!(runtime.list(NativeLocation { path: "../state".into(), ..root }), Err(BridgeError::Invalid)));
    assert_eq!(runtime.grants().unwrap().len(), 0);
    runtime.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn native_display_omits_nonportable_names_and_never_opens_symlink_target() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("external/good.txt"), b"public").unwrap();
    fs::write(fixture.0.join("external/bad:name"), b"bad").unwrap();
    fs::write(fixture.0.join("state/secret"), b"fixture secret").unwrap();
    std::os::unix::fs::symlink(fixture.0.join("state/secret"), fixture.0.join("external/link")).unwrap();
    let runtime = fixture.runtime();
    let space = runtime.attach(fixture.grant("external")).unwrap();
    let listing = runtime.list(space.root.clone()).unwrap();
    assert_eq!(listing.skipped, 1);
    assert!(listing.entries.iter().any(|entry| entry.name == "good.txt"));
    assert!(!listing.entries.iter().any(|entry| entry.name == "bad:name"));
    assert!(runtime.open_stream(NativeLocation { path: "link".into(), ..space.root }).is_err());
    assert_eq!(fs::read(fixture.0.join("state/secret")).unwrap(), b"fixture secret");
    runtime.shutdown().unwrap();
}

/// E2.8: a same-folder rename is a move without replacement; when the
/// provider picks another name it is reported, never left silently; moving
/// between folders without the host's move is "unsupported", not "access denied".
#[test]
fn saf_move_without_replacement_is_a_checked_same_folder_rename() {
    let host = Arc::new(Host::default());
    host.0.lock().unwrap().target_exists = false;
    let storage = saf(&host, true);
    // A host that cannot move documents (error 2): another folder is "unsupported", never "denied".
    assert_eq!(storage.move_no_replace(&path("a/target"), &path("target")).err().unwrap().code, ErrorCode::Unsupported);
    storage.move_no_replace(&path("source"), &path("target")).unwrap();
    host.0.lock().unwrap().chosen_name = Some("target (1)".into());
    // The fake provider keeps choosing the other name, so the object cannot be put back.
    assert_eq!(storage.move_no_replace(&path("source"), &path("target")).err().unwrap().code, ErrorCode::NeedsAttention);
    assert!(storage.capabilities().atomic_move_no_replace && storage.capabilities().directory_sync_waived);
    assert!(!storage.capabilities().durable_directory_sync);
}

/// E3.3: the report names kinds and capabilities, never folder names or paths.
#[test]
fn diagnostics_report_shows_capabilities_without_names_or_paths() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("external/Secret Folder Name")).unwrap();
    let runtime = fixture.runtime();
    runtime.attach(fixture.grant("external/Secret Folder Name")).unwrap();
    let report = runtime.diagnostics_report();
    let kinds: Vec<_> = report.spaces.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["app", "local"]);
    let local = &report.spaces[1];
    assert!(local.reachable && local.read && local.write && local.delete);
    assert_eq!(local.atomic_rename, cfg!(any(target_os = "linux", windows)), "local folder capabilities as Core reports them");
    assert!(!report.pdfium_present && !report.worker_present && !report.worker_host);
    let text = runtime.diagnostics_report_text();
    assert!(text.contains("local") && text.contains("Места"), "{text}");
    let root = fixture.0.to_string_lossy().into_owned();
    assert!(!text.contains("Secret Folder Name") && !text.contains(&root), "{text}");
    runtime.shutdown().unwrap();
}

/// E2.7: a file whose size the provider does not report (host convention
/// `bytes == u64::MAX`) is listed, marked, with size 0; operations that need
/// the exact size refuse it instead of using a wrong number.
#[test]
fn saf_file_of_unknown_size_is_listed_and_marked() {
    struct Sized;
    impl AndroidDocuments for Sized {
        fn metadata(&self, _: String, path: String) -> HostInfo {
            if path == "stream.bin" { HostInfo { error: 0, kind: 1, bytes: u64::MAX, modified_ms: Some(1000) } } else { info() }
        }
        fn list(&self, _: String, _: String, _: u32) -> HostListing {
            HostListing { error: 0, entries: vec![HostEntry { name: "a.txt".into(), info: info() },
                HostEntry { name: "stream.bin".into(), info: HostInfo { error: 0, kind: 1, bytes: u64::MAX, modified_ms: Some(1000) } }] }
        }
        fn open(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
        fn read(&self, _: u64, _: u64, _: u32) -> HostBytes { HostBytes { error: 2, data: vec![] } }
        fn file_metadata(&self, _: u64) -> HostInfo { info() }
        fn close(&self, _: u64) {}
        fn writable(&self, _: String) -> bool { false }
        fn create(&self, _: String, _: String, _: String, _: bool) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
        fn open_write(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
        fn write(&self, _: u64, _: Vec<u8>) -> u32 { 2 }
        fn finish_write(&self, _: u64) -> u32 { 2 }
        fn rename(&self, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
        fn delete(&self, _: String, _: String, _: bool) -> u32 { 2 }
        fn resolve(&self, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
        fn move_document(&self, _: String, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
    }
    let fixture = Fixture::new();
    let runtime = fixture.runtime();
    let storage = android::SafStorage { id: StorageId(Uuid::new_v4()), tree: "content://fixture.invalid/tree/s".into(), host: Arc::new(Sized), writable: false };
    assert_eq!(storage.metadata(&path("stream.bin")).err().unwrap().code, ErrorCode::Unsupported);
    assert!(storage.list(&ResourcePath::root(), 16).is_err(), "strict listing must not invent a size");
    let root = NativeLocation { storage_id: storage.id.0.to_string(), path: String::new() };
    runtime.core.register_storage(Arc::new(storage)).unwrap();
    let listing = runtime.list(root).unwrap();
    let shown: Vec<(String, u64, bool)> = listing.entries.into_iter().map(|e| (e.name, e.bytes, e.size_known)).collect();
    assert_eq!(shown, [("a.txt".to_string(), 4, true), ("stream.bin".to_string(), 0, false)]);
    assert_eq!(listing.skipped, 0);
    runtime.shutdown().unwrap();
}
