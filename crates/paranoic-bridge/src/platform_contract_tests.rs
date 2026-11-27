//! Synthetic host contracts; no device, PDFium, real grants or network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use crate::test_support::NoSealer;
use std::fs;

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
            String::new(), String::new(), vec![], Arc::new(NoSealer)).unwrap()
    }
    fn grant(&self, name: &str) -> NativeGrant {
        NativeGrant { id: Uuid::new_v4().to_string(), reference: self.0.join(name).to_str().unwrap().into() }
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
