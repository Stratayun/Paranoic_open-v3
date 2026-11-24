//! У-22: cloud uploads under Core's checkpoint (progress + cancellation), no
//! truncated file under the target name, and the process-wide spool bound.
//! One test function: the spool bound is process-wide state.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::Write, path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant}};
use paranoic_cloud::*;
use paranoic_core::{ConflictPolicy, Core, CoreConfig, CoreError, ErrorCode, JobId, JobState, Location, Operation, ResourcePath, Storage, StorageId, StorageRegistry};
use paranoic_local::{Access, LocalStorage};

struct Temp(PathBuf);
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
const TOKEN: &str = "u22-test-token-0123456789abcdefghijklmn";
struct Server(std::process::Child);
impl Drop for Server { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn hidden_uploads(root: &Path) -> Vec<String> {
    std::fs::read_dir(root).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".paranoic-up-")).collect()
}
fn rp(p: &str) -> ResourcePath { ResourcePath::parse(p).unwrap() }
fn data(len: usize, seed: u32) -> Vec<u8> { (0..len as u32).map(|i| (i.wrapping_mul(2654435761) ^ seed) as u8).collect() }

#[test] fn cloud_upload_checkpoint_and_spool_bound() {
    let temp = Temp(std::env::temp_dir().join(format!("paranoic-u22-{}", uuid::Uuid::new_v4())));
    let root = temp.0.join("served"); let spool = temp.0.join("spool");
    std::fs::create_dir_all(&root).unwrap(); std::fs::create_dir_all(&spool).unwrap();
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o700)).unwrap(); }
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = temp.0.join("config.json");
    std::fs::write(&config, serde_json::json!({ "root": root, "listen": format!("127.0.0.1:{port}"), "username": "me",
        "token_blake3": blake3::hash(TOKEN.as_bytes()).to_hex().to_string() }).to_string()).unwrap();
    let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-server");
    let _server = Server(std::process::Command::new(binary).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn().expect("paranoic-server not built"));
    let until = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() { assert!(Instant::now() < until); std::thread::sleep(Duration::from_millis(50)); }
    let dav = WebDav::new(Http::new_loopback_http().unwrap(), &format!("http://127.0.0.1:{port}/"),
        WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new(TOKEN.into()) }, true).unwrap();
    let cloud = Arc::new(CloudStorage::open(StorageId(uuid::Uuid::new_v4()), Arc::new(dav), CloudId::parse("/").unwrap(), &spool).unwrap());
    let job = || JobId(uuid::Uuid::new_v4());
    const BIG: usize = 8 << 20;

    // 1. Progress of the network upload reaches the full size; content intact.
    let content = data(BIG, 1);
    let mut stage = cloud.stage(&rp("big.bin"), job()).unwrap();
    stage.write_all(&content).unwrap();
    let mut seen = Vec::new();
    stage.publish_checked(&mut |sent| { seen.push(sent); Ok(()) }).unwrap();
    assert_eq!(std::fs::read(root.join("big.bin")).unwrap(), content);
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "progress went backwards");
    assert_eq!(seen.last(), Some(&(BIG as u64)));
    assert!(seen.iter().filter(|s| **s > 0 && **s < BIG as u64).count() >= 3, "no intermediate upload progress: {seen:?}");
    assert!(hidden_uploads(&root).is_empty());

    // 2. Cancellation in the middle of the upload: Cancelled, nothing under the
    // target name, no temporary left in the folder.
    let mut stage = cloud.stage(&rp("cancel.bin"), job()).unwrap();
    stage.write_all(&data(BIG, 2)).unwrap();
    let error = stage.publish_checked(&mut |sent| if sent >= 3 << 20 { Err(CoreError::new(ErrorCode::Cancelled, "stop")) } else { Ok(()) })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert!(!root.join("cancel.bin").exists(), "truncated file published under the target name");
    assert!(hidden_uploads(&root).is_empty(), "temporary upload left in the folder");

    // 3. Name conflict: refused before sending, existing file untouched.
    let mut stage = cloud.stage(&rp("big.bin"), job()).unwrap();
    stage.write_all(b"other").unwrap();
    assert_eq!(stage.publish_checked(&mut |_| Ok(())).unwrap_err().code, ErrorCode::AlreadyExists);
    assert_eq!(std::fs::read(root.join("big.bin")).unwrap(), content);
    assert!(hidden_uploads(&root).is_empty());

    // 4. Spool bound: a write beyond it fails; dropping the stage frees it.
    set_spool_limit(8192);
    let mut stage = cloud.stage(&rp("bounded.bin"), job()).unwrap();
    assert!(stage.write_all(&[1u8; 4096]).is_ok());
    let error = stage.write_all(&[1u8; 8192]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
    drop(stage);
    assert!(std::fs::read_dir(&spool).unwrap().next().is_none(), "spool file kept after abort");
    let mut stage = cloud.stage(&rp("bounded.bin"), job()).unwrap();
    stage.write_all(&[2u8; 8192]).unwrap();
    stage.publish().unwrap();
    assert_eq!(std::fs::read(root.join("bounded.bin")).unwrap(), vec![2u8; 8192]);
    assert!(std::fs::read_dir(&spool).unwrap().next().is_none(), "spool file kept after publish");
    set_spool_limit(4 << 30);

    // 5. Through Core: a copy job cancelled while the upload runs ends
    // Cancelled with nothing under the target name (or, if the cancel came
    // too late, a complete file) — never a truncated one.
    let local_dir = temp.0.join("local"); std::fs::create_dir_all(&local_dir).unwrap();
    let local = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &local_dir, Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new();
    registry.register(cloud.clone()).unwrap(); registry.register(local.clone()).unwrap();
    let config = CoreConfig { copy_buffer_bytes: 256 * 1024, progress_interval_bytes: 256 * 1024, ..CoreConfig::default() };
    let core = Core::open(temp.0.join("state"), config, registry).unwrap();
    let source = data(64 << 20, 3);
    std::fs::write(local_dir.join("source.bin"), &source).unwrap();
    let mut cancelled_in_upload = 0;
    for round in 0..5 {
        let name = format!("job-{round}.bin");
        let job = core.submit(Operation::CopyFile { source: Location { storage: local.id(), path: rp("source.bin") },
            target: Location { storage: cloud.id(), path: rp(&name) }, conflict: ConflictPolicy::Fail }).unwrap();
        // Remote target: total = staging + upload; past half = upload phase.
        let (mut last, until) = (0u64, Instant::now() + Duration::from_secs(60));
        let record = loop {
            let record = core.job(job.id).unwrap();
            if !matches!(record.state, JobState::Queued | JobState::Running) { break record; }
            assert!(record.bytes_done >= last, "progress went backwards");
            last = record.bytes_done;
            if let Some(total) = record.bytes_total {
                assert_eq!(total, 2 * source.len() as u64, "upload not counted in the job total");
                if record.bytes_done > total / 2 { core.cancel(job.id).unwrap(); }
            }
            assert!(Instant::now() < until, "job timed out");
            std::thread::sleep(Duration::from_millis(1));
        };
        match std::fs::read(root.join(&name)) {
            Ok(bytes) => {
                assert_eq!(bytes, source, "round {round}: truncated or altered file published");
                assert_eq!((record.state, record.bytes_done, record.bytes_total), (JobState::Succeeded, bytes.len() as u64, Some(bytes.len() as u64)));
            }
            Err(_) => { assert_eq!(record.state, JobState::Cancelled, "round {round}: {:?} {:?}", record.state, record.error); cancelled_in_upload += 1; }
        }
        assert!(hidden_uploads(&root).is_empty(), "round {round}: temporary upload left");
    }
    assert!(cancelled_in_upload > 0, "cancellation never reached the upload phase");
}
