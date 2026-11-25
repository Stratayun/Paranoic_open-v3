//! Block 3 verification against the own WebDAV server (paranoic-server):
//! server guarantees, CloudStorage through Core, transfers, synchronisation,
//! and Cryptomator in the cloud.
//! Needs `cargo build -p paranoic-server` (or PARANOIC_SERVER=<path>).
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::Write, path::{Path, PathBuf}, rc::Rc, sync::{atomic::AtomicBool, Arc}, time::{Duration, Instant}};
use paranoic_cloud::*;
use paranoic_core::{Core, CoreConfig, JobId, JobRecord, JobState, Location, MappedRecoveryAction, Operation, ResourcePath, Storage, StorageId, StorageRegistry, ConflictPolicy};
use paranoic_local::{Access, LocalStorage};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-b3-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap(); private(&p); Self(p)
    }
    fn dir(&self, name: &str) -> PathBuf { let p = self.0.join(name); std::fs::create_dir_all(&p).unwrap(); p }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn private(p: &Path) { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap(); }

const TOKEN: &str = "block3-test-token-0123456789abcdefghij";
fn server_binary() -> PathBuf {
    std::env::var("PARANOIC_SERVER").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-server"))
}
struct Server { child: std::process::Child, port: u16, root: PathBuf }
impl Drop for Server { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); } }
fn start(temp: &Temp, read_only: bool) -> Server {
    let root = temp.dir("served");
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = temp.0.join(format!("config-{port}.json"));
    std::fs::write(&config, serde_json::json!({ "root": root, "listen": format!("127.0.0.1:{port}"), "username": "me",
        "token_blake3": blake3::hash(TOKEN.as_bytes()).to_hex().to_string(), "read_only": read_only }).to_string()).unwrap();
    let child = std::process::Command::new(server_binary()).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn()
        .expect("paranoic-server not built");
    let until = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() { assert!(Instant::now() < until, "server did not start"); std::thread::sleep(Duration::from_millis(50)); }
    Server { child, port, root }
}
fn dav(server: &Server) -> WebDav {
    WebDav::new(Http::new_loopback_http().unwrap(), &format!("http://127.0.0.1:{}/", server.port),
        WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new(TOKEN.into()) }, true).unwrap()
}

/// Core with a local grant ("local"), a local spool and the WebDAV grant.
struct Env { temp: Temp, server: Server, core: Core, local: Arc<LocalStorage>, spool: Arc<LocalStorage>, cloud: Arc<CloudStorage> }
impl Env {
    fn new() -> Self {
        let temp = Temp::new();
        let server = start(&temp, false);
        let cloud_spool = temp.dir("cloud-spool"); private(&cloud_spool);
        let cloud = Arc::new(CloudStorage::open(StorageId(uuid::Uuid::new_v4()), Arc::new(dav(&server)), CloudId::parse("/").unwrap(), &cloud_spool).unwrap());
        let local = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("local"), Access::ReadWrite).unwrap());
        let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
        let mut registry = StorageRegistry::new();
        registry.register(cloud.clone()).unwrap(); registry.register(local.clone()).unwrap(); registry.register(spool.clone()).unwrap();
        let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
        Self { temp, server, core, local, spool, cloud }
    }
    fn l(&self, p: &str) -> Location { Location { storage: self.local.id(), path: ResourcePath::parse(p).unwrap() } }
    fn c(&self, p: &str) -> Location { Location { storage: self.cloud.id(), path: ResourcePath::parse(p).unwrap() } }
    fn lpath(&self, p: &str) -> PathBuf { self.temp.0.join("local").join(p) }
    fn cpath(&self, p: &str) -> PathBuf { self.server.root.join(p) }
    fn wait(&self, id: JobId) -> JobRecord {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let job = self.core.job(id).unwrap();
            if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
            assert!(Instant::now() < until, "job timed out"); std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn run(&self, op: Operation) -> JobRecord { let job = self.core.submit(op).unwrap(); self.wait(job.id) }
}
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if rel.starts_with(".paranoic") { continue; }
            if p.is_dir() { out.push((format!("{rel}/"), Vec::new())); walk(base, &p, out); } else { out.push((rel, std::fs::read(&p).unwrap())); }
        }
    }
    walk(dir, dir, &mut out); out.sort(); out
}

// ---------------- 3.5: server guarantees (raw HTTP) ----------------
#[test] fn server_auth_no_replace_trash_and_confinement() {
    let temp = Temp::new();
    let server = start(&temp, false);
    let client = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let url = |p: &str| format!("http://127.0.0.1:{}/{p}", server.port);
    let req = |m: &str, p: &str| client.request(reqwest::Method::from_bytes(m.as_bytes()).unwrap(), url(p)).basic_auth("me", Some(TOKEN));
    // Auth: none, wrong token (delayed), right token, bearer.
    assert_eq!(client.get(url("")).send().unwrap().status(), 401);
    let started = Instant::now();
    assert_eq!(client.get(url("")).basic_auth("me", Some("wrong")).send().unwrap().status(), 401);
    assert!(started.elapsed() >= Duration::from_millis(400), "failed auth not delayed");
    assert_eq!(client.get(url("")).basic_auth("someone", Some(TOKEN)).send().unwrap().status(), 401, "user name must match");
    assert!(client.request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url("")).header("Depth", "0").bearer_auth(TOKEN).send().unwrap().status().is_success());
    // Create-new PUT never replaces.
    assert!(req("PUT", "a.txt").body("first").send().unwrap().status().is_success());
    let again = req("PUT", "a.txt").header("If-None-Match", "*").body("second").send().unwrap().status();
    assert_eq!(again, 412);
    assert_eq!(std::fs::read(server.root.join("a.txt")).unwrap(), b"first");
    // MOVE / COPY never replace, even with Overwrite: T.
    assert!(req("PUT", "b.txt").body("other").send().unwrap().status().is_success());
    for method in ["MOVE", "COPY"] {
        let s = req(method, "b.txt").header("Destination", url("a.txt")).header("Overwrite", "T").send().unwrap().status();
        assert!(!s.is_success(), "{method} replaced an existing file: {s}");
        assert_eq!(std::fs::read(server.root.join("a.txt")).unwrap(), b"first");
    }
    // DELETE moves to the hidden trash; the trash is not reachable over WebDAV.
    assert!(req("DELETE", "b.txt").send().unwrap().status().is_success());
    assert!(!server.root.join("b.txt").exists());
    let trashed: Vec<_> = walk_files(&server.root.join(".paranoic-trash"));
    assert!(trashed.iter().any(|p| std::fs::read(p).unwrap() == b"other"), "deleted file not kept in trash");
    let listing = req("PROPFIND", "").header("Depth", "1").send().unwrap().text().unwrap();
    assert!(!listing.contains(".paranoic-trash"), "trash listed");
    assert!(!req("GET", ".paranoic-trash/").send().unwrap().status().is_success());
    assert!(!req("PUT", ".paranoic-trash/x").body("x").send().unwrap().status().is_success());
    // Escapes out of the root.
    std::fs::write(temp.0.join("outside.txt"), b"secret").unwrap();
    for path in ["../outside.txt", "%2e%2e/outside.txt", "..%2foutside.txt"] {
        let r = req("GET", path).send().unwrap();
        let status = r.status(); let body = r.text().unwrap_or_default();
        assert!(!body.contains("secret"), "{path} escaped the root ({status})");
    }
    // A symlink inside the root must not lead outside.
    std::os::unix::fs::symlink(temp.0.join("outside.txt"), server.root.join("link")).unwrap();
    let body = req("GET", "link").send().unwrap().text().unwrap_or_default();
    assert!(!body.contains("secret"), "symlink followed outside the root");
    // A symlinked directory: neither listed nor readable through.
    std::fs::create_dir(temp.0.join("outside-dir")).unwrap();
    std::fs::write(temp.0.join("outside-dir/hidden-name.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(temp.0.join("outside-dir"), server.root.join("dirlink")).unwrap();
    let listing = req("PROPFIND", "dirlink/").header("Depth", "1").send().unwrap().text().unwrap_or_default();
    assert!(!listing.contains("hidden-name"), "directory symlink listed outside the root");
    let body = req("GET", "dirlink/hidden-name.txt").send().unwrap().text().unwrap_or_default();
    assert!(!body.contains("secret"), "read through a directory symlink");
    assert!(!req("PUT", "dirlink/new.txt").body("x").send().unwrap().status().is_success(), "write through a directory symlink");
    assert!(!temp.0.join("outside-dir/new.txt").exists());
    // У-23: a local user swaps a folder for a symlink while requests run. The
    // check-then-use window must not exist: never read or write outside.
    std::fs::write(temp.0.join("outside-dir/inside.txt"), b"secret").unwrap();
    let swap = server.root.join("swap");
    let (real, link) = (server.root.join("swap-real"), server.root.join("swap-link"));
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("inside.txt"), b"public").unwrap();
    std::os::unix::fs::symlink(temp.0.join("outside-dir"), &link).unwrap();
    std::fs::rename(&real, &swap).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let swapper = {
        let (stop, swap, real, link) = (stop.clone(), swap.clone(), real.clone(), link.clone());
        std::thread::spawn(move || while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            // swap = real dir -> swap = symlink -> back (renames are atomic).
            let _ = std::fs::rename(&swap, &real);
            let _ = std::fs::rename(&link, &swap);
            let _ = std::fs::rename(&swap, &link);
            let _ = std::fs::rename(&real, &swap);
        })
    };
    for i in 0..400 {
        let body = req("GET", "swap/inside.txt").send().unwrap().text().unwrap_or_default();
        assert!(!body.contains("secret"), "read outside the root during a swap (round {i})");
        let listing = req("PROPFIND", "swap/").header("Depth", "1").send().unwrap().text().unwrap_or_default();
        assert!(!listing.contains("hidden-name"), "listed outside the root during a swap (round {i})");
        let _ = req("PUT", &format!("swap/race-{i}.txt")).body("x").send();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    swapper.join().unwrap();
    let escaped: Vec<_> = std::fs::read_dir(temp.0.join("outside-dir")).unwrap()
        .filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("race-")).collect();
    assert!(escaped.is_empty(), "{} files written outside the root during a swap", escaped.len());
}
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) { for e in entries { let p = e.unwrap().path(); if p.is_dir() { out.extend(walk_files(&p)); } else { out.push(p); } } }
    out
}

#[test] fn server_read_only_mode_and_tls_requirement() {
    let temp = Temp::new();
    let server = start(&temp, true);
    std::fs::write(server.root.join("r.txt"), b"readable").unwrap();
    let client = reqwest::blocking::Client::new();
    let url = |p: &str| format!("http://127.0.0.1:{}/{p}", server.port);
    assert_eq!(client.get(url("r.txt")).basic_auth("me", Some(TOKEN)).send().unwrap().text().unwrap(), "readable");
    for method in ["PUT", "DELETE", "MKCOL", "MOVE"] {
        let s = client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url("r.txt")).basic_auth("me", Some(TOKEN))
            .header("Destination", url("moved.txt")).body("x").send().unwrap().status();
        assert!(!s.is_success(), "{method} allowed in read-only mode");
    }
    assert_eq!(std::fs::read(server.root.join("r.txt")).unwrap(), b"readable");
    // Non-loopback listener without TLS: refused at start.
    let config = temp.0.join("public.json");
    std::fs::write(&config, serde_json::json!({ "root": server.root, "listen": "0.0.0.0:0", "username": "me",
        "token_blake3": blake3::hash(TOKEN.as_bytes()).to_hex().to_string() }).to_string()).unwrap();
    let status = std::process::Command::new(server_binary()).arg("run").arg(&config).stderr(std::process::Stdio::null()).status().unwrap();
    assert!(!status.success(), "server started on a public address without TLS");
    // The WebDAV client refuses plain http to a non-loopback host.
    assert!(WebDav::new(Http::new_loopback_http().unwrap(), "http://example.com/", WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new("x".into()) }, true).is_err());
}

// ---------------- 3.1 / 3.7: CloudStorage through Core ----------------
#[test] fn cloud_grant_through_core_trees_conflicts_and_trash() {
    let env = Env::new();
    let caps = env.cloud.capabilities();
    assert!(caps.read && caps.write && caps.delete && caps.atomic_move_no_replace && caps.conditional_move_source, "{caps:?}");
    // A local tree with nested folders, an empty file and a 300 KB file.
    std::fs::create_dir_all(env.lpath("tree/sub/deeper")).unwrap();
    std::fs::write(env.lpath("tree/a.txt"), b"alpha").unwrap();
    std::fs::write(env.lpath("tree/sub/empty"), b"").unwrap();
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(env.lpath("tree/sub/deeper/big.bin"), &big).unwrap();
    std::fs::write(env.lpath("tree/Документ.md"), "# текст").unwrap();
    let local_tree = tree(&env.lpath("tree"));
    // Upload (copy local -> cloud), verify byte-identical on the server's disk.
    let r = env.run(Operation::CopyDirectory { source: env.l("tree"), target: env.c("tree"), conflict: ConflictPolicy::Fail });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert_eq!(tree(&env.cpath("tree")), local_tree);
    // Listing through Core.
    let names: Vec<String> = env.core.list(&env.c("tree")).unwrap().into_iter().map(|e| e.location.path.as_str().to_owned()).collect();
    assert!(names.contains(&"tree/Документ.md".to_string()) && names.contains(&"tree/sub".to_string()), "{names:?}");
    // Existing destination: no merge, nothing changed.
    std::fs::write(env.lpath("tree/a.txt"), b"ALPHA CHANGED").unwrap();
    let r = env.run(Operation::CopyDirectory { source: env.l("tree"), target: env.c("tree"), conflict: ConflictPolicy::Fail });
    assert_ne!(r.state, JobState::Succeeded);
    assert_eq!(std::fs::read(env.cpath("tree/a.txt")).unwrap(), b"alpha");
    // Single file: create-new refuses, KeepBoth adds a copy.
    let r = env.run(Operation::CopyFile { source: env.l("tree/a.txt"), target: env.c("tree/a.txt"), conflict: ConflictPolicy::Fail });
    assert_ne!(r.state, JobState::Succeeded);
    let r = env.run(Operation::CopyFile { source: env.l("tree/a.txt"), target: env.c("tree/a.txt"), conflict: ConflictPolicy::KeepBoth });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert_eq!(std::fs::read(env.cpath("tree/a.txt")).unwrap(), b"alpha");
    assert_eq!(tree(&env.cpath("tree")).iter().filter(|(_, d)| d == b"ALPHA CHANGED").count(), 1);
    // Rename inside the cloud (atomic, no replace).
    let r = env.run(Operation::Move { source: env.c("tree/Документ.md"), target: env.c("tree/renamed.md") });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert!(env.cpath("tree/renamed.md").exists() && !env.cpath("tree/Документ.md").exists());
    let r = env.run(Operation::Move { source: env.c("tree/renamed.md"), target: env.c("tree/a.txt") });
    assert_ne!(r.state, JobState::Succeeded, "move replaced an existing file");
    // Download the whole tree back (cloud -> local) and compare.
    let r = env.run(Operation::CopyDirectory { source: env.c("tree"), target: env.l("back"), conflict: ConflictPolicy::Fail });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert_eq!(tree(&env.lpath("back")), tree(&env.cpath("tree")));
    // Move a tree across storages (cloud -> local): verified copy, then the cloud source goes to the trash.
    let r = env.run(Operation::MoveDirectoryAcross { source: env.c("tree/sub"), target: env.l("moved-sub") });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert!(!env.cpath("tree/sub").exists());
    assert_eq!(std::fs::read(env.lpath("moved-sub/deeper/big.bin")).unwrap(), big);
    assert!(walk_files(&env.server.root.join(".paranoic-trash")).iter().any(|p| std::fs::read(p).unwrap() == big), "removed source not in the server trash");
    // Delete a file: trash, not permanent removal.
    let r = env.run(Operation::DeleteFile { target: env.c("tree/renamed.md") });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert!(!env.cpath("tree/renamed.md").exists());
    // A file changed on the server between planning and removal is not removed.
    std::fs::write(env.lpath("m.txt"), b"to move").unwrap();
    let r = env.run(Operation::MoveFileAcross { source: env.l("m.txt"), target: env.c("m.txt") });
    assert_eq!(r.state, JobState::Succeeded, "{r:?}");
    assert!(!env.lpath("m.txt").exists() && std::fs::read(env.cpath("m.txt")).unwrap() == b"to move");
}

#[test] fn transfers_verify_content() {
    let env = Env::new();
    let provider = dav(&env.server);
    let data: Vec<u8> = (0..100_000u32).map(|i| (i * 13 % 256) as u8).collect();
    std::fs::write(env.lpath("up.bin"), &data).unwrap();
    let entry = upload_from_core(&env.core, &env.l("up.bin"), &provider, &CloudId::parse("/").unwrap(), "up.bin", |_| Ok(())).unwrap();
    assert_eq!(std::fs::read(env.cpath("up.bin")).unwrap(), data);
    use sha2::Digest;
    let digest: [u8; 32] = sha2::Sha256::digest(&data).into();
    verify_remote(&provider, &entry, data.len() as u64, &digest, &mut |_| Ok(())).unwrap();
    let mut wrong = digest; wrong[0] ^= 1;
    assert!(verify_remote(&provider, &entry, data.len() as u64, &wrong, &mut |_| Ok(())).is_err());
    // Download into a new local file; a changed remote version is refused.
    let got = download_to_storage(&provider, &entry, env.local.as_ref(), &ResourcePath::parse("down.bin").unwrap(), |_| Ok(())).unwrap();
    assert_eq!(got, digest); assert_eq!(std::fs::read(env.lpath("down.bin")).unwrap(), data);
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.cpath("up.bin"), b"replaced on the server").unwrap();
    assert!(download_to_storage(&provider, &entry, env.local.as_ref(), &ResourcePath::parse("down2.bin").unwrap(), |_| Ok(())).is_err());
    assert!(!env.lpath("down2.bin").exists());
    // Existing local target: never overwritten.
    let fresh = provider.metadata(&entry.id).unwrap();
    assert!(download_to_storage(&provider, &fresh, env.local.as_ref(), &ResourcePath::parse("down.bin").unwrap(), |_| Ok(())).is_err());
    assert_eq!(std::fs::read(env.lpath("down.bin")).unwrap(), data);
}

// ---------------- 3.8: synchronisation ----------------
fn sync_round(env: &Env, baseline: &mut SyncBaseline, mode: SyncMode) -> SyncPlan {
    let mut plan = baseline.plan(&env.core, &env.cloud, SyncOptions { mode, trash_remote_deletions: false }, &AtomicBool::new(false)).unwrap();
    baseline.submit(&env.core, &env.cloud, &mut plan).unwrap();
    for action in &plan.actions { if let Some(job) = action.job { let r = env.wait(job); assert_eq!(r.state, JobState::Succeeded, "{:?}: {r:?}", action.kind); } }
    baseline.commit(&env.core, &env.cloud, &plan).unwrap();
    plan
}
fn kinds(plan: &SyncPlan) -> Vec<(String, SyncActionKind)> {
    let mut v: Vec<_> = plan.actions.iter().map(|a| (a.path.clone(), a.kind)).collect(); v.sort_by(|a, b| a.0.cmp(&b.0)); v
}

#[test] fn two_way_sync_conflicts_and_backup_mode() {
    let env = Env::new();
    std::fs::create_dir_all(env.lpath("sync/docs")).unwrap();
    std::fs::create_dir_all(env.cpath("remote")).unwrap();
    std::fs::write(env.lpath("sync/docs/a.txt"), b"local a").unwrap();
    std::fs::create_dir_all(env.cpath("remote/inbox")).unwrap();
    std::fs::write(env.cpath("remote/inbox/b.txt"), b"remote b").unwrap();
    let mut base = SyncBaseline::new(env.l("sync"), env.c("remote"));
    // First round: each side gets the other's files.
    sync_round(&env, &mut base, SyncMode::TwoWay);
    assert_eq!(std::fs::read(env.cpath("remote/docs/a.txt")).unwrap(), b"local a");
    assert_eq!(std::fs::read(env.lpath("sync/inbox/b.txt")).unwrap(), b"remote b");
    // Nothing changed: nothing to do.
    let quiet = sync_round(&env, &mut base, SyncMode::TwoWay);
    assert!(quiet.actions.iter().all(|a| a.kind == SyncActionKind::Adopt || a.job.is_none()), "{:?}", kinds(&quiet));
    // One-sided edits travel; both-sided edit becomes a conflict, nothing overwritten.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.lpath("sync/docs/a.txt"), b"local a v2 (longer)").unwrap();
    std::fs::write(env.cpath("remote/inbox/b.txt"), b"remote b v2 (longer)").unwrap();
    sync_round(&env, &mut base, SyncMode::TwoWay);
    assert_eq!(std::fs::read(env.cpath("remote/docs/a.txt")).unwrap(), b"local a v2 (longer)");
    assert_eq!(std::fs::read(env.lpath("sync/inbox/b.txt")).unwrap(), b"remote b v2 (longer)");
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.lpath("sync/docs/a.txt"), b"edited locally").unwrap();
    std::fs::write(env.cpath("remote/docs/a.txt"), b"edited remotely, other size").unwrap();
    let plan = sync_round(&env, &mut base, SyncMode::TwoWay);
    assert!(plan.actions.iter().any(|a| a.kind == SyncActionKind::ConflictCopy), "{:?}", kinds(&plan));
    assert_eq!(std::fs::read(env.lpath("sync/docs/a.txt")).unwrap(), b"edited locally", "local version overwritten");
    assert_eq!(std::fs::read(env.cpath("remote/docs/a.txt")).unwrap(), b"edited remotely, other size", "remote version overwritten");
    assert!(tree(&env.lpath("sync")).iter().any(|(_, d)| d == b"edited remotely, other size"), "cloud copy not saved locally");
    assert_eq!(base.conflicts().collect::<Vec<_>>(), ["docs/a.txt"]);
    // Unresolved conflict stays out of sync; the user keeps the local version.
    let still = sync_round(&env, &mut base, SyncMode::TwoWay);
    assert!(!still.actions.iter().any(|a| a.path == "docs/a.txt" && a.job.is_some()), "conflict synced before a decision");
    base.resolve_conflict(&env.core, "docs/a.txt", ConflictChoice::KeepLocal).unwrap();
    sync_round(&env, &mut base, SyncMode::TwoWay);
    assert_eq!(std::fs::read(env.cpath("remote/docs/a.txt")).unwrap(), b"edited locally");
    // Same size, same second (WebDAV dates have 1 s resolution): only the
    // provider revision (ETag) shows the change (register У-24).
    let file = env.cpath("remote/docs/a.txt");
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    let seconds = before.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let sub = before.duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis();
    std::fs::write(&file, b"EDITED LOCALLY").unwrap(); // same length as "edited locally"
    let same_second = std::time::UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_millis(if sub == 999 { 1 } else { 999 });
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(same_second).unwrap();
    sync_round(&env, &mut base, SyncMode::TwoWay);
    assert_eq!(std::fs::read(env.lpath("sync/docs/a.txt")).unwrap(), b"EDITED LOCALLY", "same-size same-second remote edit missed");

    // Baseline is sealed and bound to its pair.
    let key = paranoic_crypto::SecretKey::generate().unwrap();
    let sealed = base.seal(&key).unwrap();
    assert!(SyncBaseline::open(&key, &sealed, &env.l("sync"), &env.c("remote")).is_ok());
    assert!(SyncBaseline::open(&key, &sealed, &env.l("other"), &env.c("remote")).is_err());
    assert!(!String::from_utf8_lossy(&sealed).contains("docs"), "baseline names in plaintext");
    // Deletions are reported, not propagated without the option.
    std::fs::remove_file(env.lpath("sync/inbox/b.txt")).unwrap();
    let plan = sync_round(&env, &mut base, SyncMode::TwoWay);
    assert!(plan.reports.iter().any(|r| matches!(r, SyncReport::DeletedLocally { .. })), "{:?}", plan.reports);
    assert!(env.cpath("remote/inbox/b.txt").exists(), "remote deleted without the option");
    // Backup mode: never downloads over local data; cloud changes are reported.
    let mut backup = SyncBaseline::new(env.l("backup"), env.c("backup"));
    std::fs::create_dir_all(env.lpath("backup")).unwrap(); std::fs::create_dir_all(env.cpath("backup")).unwrap();
    std::fs::write(env.lpath("backup/keep.txt"), b"local only").unwrap();
    std::fs::write(env.cpath("backup/cloud-only.txt"), b"from cloud").unwrap();
    let plan = sync_round(&env, &mut backup, SyncMode::Backup);
    assert_eq!(std::fs::read(env.cpath("backup/keep.txt")).unwrap(), b"local only");
    assert!(!env.lpath("backup/cloud-only.txt").exists(), "backup mode downloaded");
    assert!(plan.reports.iter().any(|r| matches!(r, SyncReport::RemoteOnly { .. })), "{:?}", plan.reports);
}

/// Server trash retention: the owner empties it (all or older than N days) on
/// the server machine; with `trash_days` the server removes old items itself.
#[test] fn server_trash_retention_and_empty_command() {
    let temp = Temp::new();
    let server = start(&temp, false);
    let trash = server.root.join(".paranoic-trash");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let bucket = |age_days: u128| {
        let path = trash.join(format!("{}-{}", now - age_days * 86_400_000, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).unwrap(); std::fs::write(path.join("deleted.txt"), b"old").unwrap(); path
    };
    let (old, fresh) = (bucket(3), bucket(0));
    std::fs::write(trash.join("not-a-bucket"), b"keep").unwrap();
    let config = temp.0.join(format!("config-{}.json", server.port));
    let empty = |extra: &[&str]| std::process::Command::new(server_binary()).arg("empty-trash").arg(&config).args(extra).output().unwrap();
    assert!(empty(&["--older-than", "1"]).status.success());
    assert!(!old.exists(), "old item kept");
    assert!(fresh.exists(), "recent item removed");
    assert!(empty(&[]).status.success());
    assert!(!fresh.exists(), "empty-trash left an item");
    assert!(trash.join("not-a-bucket").exists(), "foreign entry removed");
    drop(server);
    // Retention at start.
    let old = bucket(5);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = temp.0.join("retention.json");
    std::fs::write(&config, serde_json::json!({ "root": temp.0.join("served"), "listen": format!("127.0.0.1:{port}"), "username": "me",
        "token_blake3": blake3::hash(TOKEN.as_bytes()).to_hex().to_string(), "trash_days": 1 }).to_string()).unwrap();
    let mut child = std::process::Command::new(server_binary()).arg("run").arg(&config).stderr(std::process::Stdio::null()).spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while old.exists() && Instant::now() < until { std::thread::sleep(Duration::from_millis(50)); }
    let _ = child.kill(); let _ = child.wait();
    assert!(!old.exists(), "trash_days did not remove an old item");
}

/// У-35: only a server that declares a recoverable trash enables deletion.
#[test] fn trash_is_confirmed_only_by_the_server() {
    let temp = Temp::new();
    let writable = start(&temp, false);
    let probe = |server: &Server| WebDav::new(Http::new_loopback_http().unwrap(), &format!("http://127.0.0.1:{}/", server.port),
        WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new(TOKEN.into()) }, false).unwrap().probe_trash();
    assert!(probe(&writable), "own server trash not detected");
    let read_only = start(&temp, true);
    assert!(!probe(&read_only), "read-only server announced a trash");
    let wrong = WebDav::new(Http::new_loopback_http().unwrap(), &format!("http://127.0.0.1:{}/", writable.port),
        WebDavCredentials { username: "me".into(), password: zeroize::Zeroizing::new("wrong".into()) }, false).unwrap();
    assert!(!wrong.probe_trash(), "unauthenticated answer accepted");
}

/// A storage that reports a chosen binding key (to seed the journal).
struct Keyed(Arc<CloudStorage>, String);
impl Storage for Keyed {
    fn id(&self) -> StorageId { self.0.id() }
    fn binding_key(&self) -> paranoic_core::Result<String> { Ok(self.1.clone()) }
    fn domain(&self) -> paranoic_core::StorageDomain { self.0.domain() }
    fn capabilities(&self) -> paranoic_core::Capabilities { self.0.capabilities() }
    fn metadata(&self, p: &ResourcePath) -> paranoic_core::Result<paranoic_core::Metadata> { self.0.metadata(p) }
    fn list(&self, p: &ResourcePath, l: usize) -> paranoic_core::Result<Vec<paranoic_core::DirectoryEntry>> { self.0.list(p, l) }
    fn open_read(&self, p: &ResourcePath) -> paranoic_core::Result<Box<dyn paranoic_core::ReadHandle>> { self.0.open_read(p) }
    fn stage(&self, t: &ResourcePath, j: JobId) -> paranoic_core::Result<Box<dyn paranoic_core::StagedWrite>> { self.0.stage(t, j) }
    fn create_directory(&self, p: &ResourcePath) -> paranoic_core::Result<()> { self.0.create_directory(p) }
    fn move_no_replace(&self, s: &ResourcePath, t: &ResourcePath) -> paranoic_core::Result<()> { self.0.move_no_replace(s, t) }
    fn delete_file(&self, p: &ResourcePath) -> paranoic_core::Result<()> { self.0.delete_file(p) }
    fn delete_empty_directory(&self, p: &ResourcePath) -> paranoic_core::Result<()> { self.0.delete_empty_directory(p) }
}

/// У-38: a grant bound before the identity upgrade keeps its storage ID (v1 →
/// v2 once); any other stored binding is still refused; no way back to v1.
#[test] fn webdav_binding_upgrades_only_from_its_own_v1_identity() {
    let env = Env::new();
    let fresh = || {
        let spool = env.temp.dir(&format!("spool-{}", uuid::Uuid::new_v4().simple())); private(&spool);
        Arc::new(CloudStorage::open(StorageId(uuid::Uuid::new_v4()), Arc::new(dav(&env.server)), CloudId::parse("/").unwrap(), &spool).unwrap())
    };
    let cloud = fresh();
    let v1 = format!("cloud:{:?}:me@127.0.0.1/:/", cloud.provider().kind());
    assert_eq!(cloud.legacy_binding_keys().unwrap(), [v1.clone()]);
    env.core.register_storage(Arc::new(Keyed(cloud.clone(), v1.clone()))).unwrap();
    env.core.unregister_storage(cloud.id()).unwrap();
    env.core.register_storage(cloud.clone()).expect("v1 binding of the same connection must upgrade");
    assert_eq!(env.core.storage_binding_fingerprint(cloud.id()).unwrap(), blake3::hash(cloud.binding_key().unwrap().as_bytes()).to_hex().to_string());
    env.core.unregister_storage(cloud.id()).unwrap();
    let back = env.core.register_storage(Arc::new(Keyed(cloud.clone(), v1))).unwrap_err();
    assert_eq!(back.code, paranoic_core::ErrorCode::PermissionDenied, "downgraded to v1");
    let other = fresh();
    env.core.register_storage(Arc::new(Keyed(other.clone(), "cloud:WebDav:me@elsewhere/:/".into()))).unwrap();
    env.core.unregister_storage(other.id()).unwrap();
    let refused = env.core.register_storage(other).unwrap_err();
    assert_eq!(refused.code, paranoic_core::ErrorCode::PermissionDenied, "foreign binding accepted");
}

/// У-24: a remote edit with the same size within the same second between
/// planning and submission is caught by the provider revision; nothing is
/// overwritten.
#[test] fn sync_submit_rechecks_remote_revision() {
    let env = Env::new();
    std::fs::create_dir_all(env.lpath("s")).unwrap();
    std::fs::create_dir_all(env.cpath("r")).unwrap();
    std::fs::write(env.lpath("s/a.txt"), b"same len").unwrap();
    let mut base = SyncBaseline::new(env.l("s"), env.c("r"));
    sync_round(&env, &mut base, SyncMode::TwoWay);
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.lpath("s/a.txt"), b"local edit, longer").unwrap();
    let mut plan = base.plan(&env.core, &env.cloud, SyncOptions { mode: SyncMode::TwoWay, trash_remote_deletions: false }, &AtomicBool::new(false)).unwrap();
    assert!(plan.actions.iter().any(|a| a.path == "a.txt" && a.kind == SyncActionKind::UploadReplace), "{:?}", kinds(&plan));
    let file = env.cpath("r/a.txt");
    let before = std::fs::metadata(&file).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
    // Same second as planned (WebDAV dates have 1 s resolution), other sub-second.
    let sub = before.subsec_millis();
    let same_second = std::time::UNIX_EPOCH + Duration::from_secs(before.as_secs()) + Duration::from_millis(if sub == 999 { 1 } else { 999 });
    std::fs::write(&file, b"SAME LEN").unwrap();
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(same_second).unwrap();
    base.submit(&env.core, &env.cloud, &mut plan).unwrap();
    assert!(plan.reports.iter().any(|r| matches!(r, SyncReport::ChangedBeforeSubmit { path } if path == "a.txt")), "{:?}", plan.reports);
    for action in &plan.actions { if let Some(job) = action.job { env.wait(job); } }
    assert_eq!(std::fs::read(&file).unwrap(), b"SAME LEN", "remote edit overwritten");
}

/// У-24: the job itself refuses a stale provider revision even when size and
/// time still match; with the current revision it runs.
#[test] fn copy_job_checks_provider_revision() {
    let env = Env::new();
    std::fs::create_dir_all(env.cpath("r")).unwrap();
    std::fs::write(env.cpath("r/a.txt"), b"remote").unwrap();
    std::fs::write(env.lpath("a.txt"), b"local!").unwrap();
    let local = env.core.metadata(&env.l("a.txt")).unwrap();
    let remote = env.core.metadata(&env.c("r/a.txt")).unwrap();
    let current = env.cloud.revision(&ResourcePath::parse("r/a.txt").unwrap()).unwrap();
    assert!(current.is_some(), "server gives no ETag");
    let upload = |revision: Option<String>| Operation::CopyFileExpected { source: env.l("a.txt"), target: env.c("r/a.txt"),
        expected_source: local.clone(), expected_target: Some(remote.clone()), keep_both: false,
        expected_source_revision: None, expected_target_revision: revision };
    let stale = env.run(upload(Some("\"stale\"".into())));
    assert_eq!(stale.state, JobState::Failed, "{stale:?}");
    assert_eq!(std::fs::read(env.cpath("r/a.txt")).unwrap(), b"remote", "overwritten despite a stale revision");
    let download = Operation::CopyFileExpected { source: env.c("r/a.txt"), target: env.l("b.txt"),
        expected_source: remote.clone(), expected_target: None, keep_both: false,
        expected_source_revision: Some("\"stale\"".into()), expected_target_revision: None };
    assert_eq!(env.run(download).state, JobState::Failed);
    assert!(!env.lpath("b.txt").exists(), "downloaded a stale revision");
    let ok = env.run(upload(current));
    assert_eq!(ok.state, JobState::Succeeded, "{ok:?}");
    assert_eq!(std::fs::read(env.cpath("r/a.txt")).unwrap(), b"local!");
}

/// External audit A-5: another client writes the same length to the remote
/// file after our upload but before commit. The baseline must not record
/// that version as ours; the next plan still sees a difference.
#[test] fn sync_commit_does_not_adopt_a_remote_write_after_the_upload() {
    let env = Env::new();
    std::fs::create_dir_all(env.lpath("s")).unwrap();
    std::fs::create_dir_all(env.cpath("r")).unwrap();
    std::fs::write(env.lpath("s/a.txt"), b"ours 123").unwrap();
    let mut base = SyncBaseline::new(env.l("s"), env.c("r"));
    let mut plan = base.plan(&env.core, &env.cloud, SyncOptions { mode: SyncMode::TwoWay, trash_remote_deletions: false }, &AtomicBool::new(false)).unwrap();
    base.submit(&env.core, &env.cloud, &mut plan).unwrap();
    for action in &plan.actions { if let Some(job) = action.job { assert_eq!(env.wait(job).state, JobState::Succeeded); } }
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.cpath("r/a.txt"), b"them 456").unwrap();
    base.commit(&env.core, &env.cloud, &plan).unwrap();
    let next = base.plan(&env.core, &env.cloud, SyncOptions { mode: SyncMode::TwoWay, trash_remote_deletions: false }, &AtomicBool::new(false)).unwrap();
    assert!(next.actions.iter().any(|a| a.path == "a.txt" && a.kind != SyncActionKind::Adopt)
        || next.reports.iter().any(|r| format!("{r:?}").contains("a.txt")),
        "remote write adopted as synced: {:?}", kinds(&next));
}

/// External audit A-4: a trash job deletes only the revision it was planned
/// for; a version written meanwhile (same size and time) is kept.
#[test] fn trash_job_deletes_only_the_planned_revision() {
    let env = Env::new();
    std::fs::create_dir_all(env.cpath("r")).unwrap();
    std::fs::write(env.cpath("r/a.txt"), b"remote").unwrap();
    let remote = env.core.metadata(&env.c("r/a.txt")).unwrap();
    let current = env.cloud.revision(&ResourcePath::parse("r/a.txt").unwrap()).unwrap();
    assert!(current.is_some(), "server gives no ETag");
    let trash = |revision: Option<String>| Operation::DeleteFileExpected { target: env.c("r/a.txt"), expected: remote.clone(),
        expected_revision: revision };
    let stale = env.run(trash(Some("\"stale\"".into())));
    assert_eq!(stale.state, JobState::Failed, "{stale:?}");
    assert!(env.cpath("r/a.txt").exists(), "deleted a version other than the planned one");
    let ok = env.run(trash(current));
    assert_eq!(ok.state, JobState::Succeeded, "{ok:?}");
    assert!(!env.cpath("r/a.txt").exists());
}

/// У-36: an edit made after the upload job ran but before commit must not be
/// recorded as synchronized; the next plan uploads it again.
#[test] fn sync_commit_keeps_edits_made_after_the_job() {
    let env = Env::new();
    std::fs::create_dir_all(env.lpath("s")).unwrap();
    std::fs::create_dir_all(env.cpath("r")).unwrap();
    std::fs::write(env.lpath("s/a.txt"), b"v1").unwrap();
    let mut base = SyncBaseline::new(env.l("s"), env.c("r"));
    sync_round(&env, &mut base, SyncMode::TwoWay);
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.lpath("s/a.txt"), b"v2 uploaded").unwrap();
    let mut plan = base.plan(&env.core, &env.cloud, SyncOptions { mode: SyncMode::TwoWay, trash_remote_deletions: false }, &AtomicBool::new(false)).unwrap();
    base.submit(&env.core, &env.cloud, &mut plan).unwrap();
    for action in &plan.actions { if let Some(job) = action.job { assert_eq!(env.wait(job).state, JobState::Succeeded); } }
    // Edited again before the baseline is committed.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(env.lpath("s/a.txt"), b"v3 edited before commit").unwrap();
    base.commit(&env.core, &env.cloud, &plan).unwrap();
    let next = sync_round(&env, &mut base, SyncMode::TwoWay);
    assert!(next.actions.iter().any(|a| a.path == "a.txt" && a.kind == SyncActionKind::UploadReplace), "{:?}", kinds(&next));
    assert_eq!(std::fs::read(env.cpath("r/a.txt")).unwrap(), b"v3 edited before commit");
}

// ---------------- 3.9: Cryptomator in the cloud ----------------
// The production Cryptomator KDF gate returns Busy for concurrent derivations
// (by design, see У-44). Tests that derive keys hold this lock for the whole scenario.
static KDF_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[derive(Default)] struct Out { bytes: Vec<u8> }
struct Sink(Vec<u8>, Rc<RefCell<Out>>);
impl Write for Sink { fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.extend_from_slice(b); Ok(b.len()) } fn flush(&mut self) -> std::io::Result<()> { Ok(()) } }
impl paranoic_crypto::StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { self.1.borrow_mut().bytes = self.0.clone(); Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}
fn sink() -> (Box<dyn paranoic_crypto::StreamSink>, Rc<RefCell<Out>>) { let o = Rc::new(RefCell::new(Out::default())); (Box::new(Sink(Vec::new(), o.clone())), o) }

#[test] fn cryptomator_vault_on_webdav() {
    use paranoic_formats::*;
    let _kdf = KDF_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    let env = Env::new();
    check_vault_support(&env.cloud, true).unwrap();
    let password = || zeroize::Zeroizing::new("cloud vault password".to_string());
    let bootstrap = CryptomatorVault::prepare_create(password(), || Ok(())).unwrap();
    let job = bootstrap.submit(&env.core, env.c("Vault"), JobId(uuid::Uuid::new_v4())).unwrap();
    let created = env.wait(job.id); assert_eq!(created.state, JobState::Succeeded, "{created:?}");
    let reader = CryptomatorReader::open(env.cloud.clone(), ResourcePath::parse("Vault").unwrap(), password(), Duration::from_secs(300), || Ok(())).unwrap();
    let dir = reader.prepare_directory(&[], "folder", || Ok(())).unwrap();
    let job = reader.submit_directory(&env.core, &dir, JobId(uuid::Uuid::new_v4())).unwrap();
    let made = env.wait(job.id);
    if made.state != JobState::Succeeded { for p in walk_all(&env.cpath("Vault")) { eprintln!("DBG {p}"); } }
    assert_eq!(made.state, JobState::Succeeded, "{:?}", made.error);
    for name in ["short.txt".to_string(), "x".repeat(180)] {
        let (s, spool) = sink();
        let prepared = reader.prepare_file_import(&[], &name, &b"cloud secret data"[..], s, Default::default(), |_| Ok(())).unwrap();
        std::fs::write(env.temp.0.join("spool/import"), &spool.borrow().bytes).unwrap();
        let job = reader.submit_file_import(&env.core, &prepared, Location { storage: env.spool.id(), path: ResourcePath::parse("import").unwrap() }, JobId(uuid::Uuid::new_v4())).unwrap();
        let r = env.wait(job.id); assert_eq!(r.state, JobState::Succeeded, "{r:?}");
        std::fs::remove_file(env.temp.0.join("spool/import")).unwrap();
        // Move into the folder with a new name (shortened wrapper => RenameMapped on the cloud).
        let target = format!("{name}-moved");
        let plan = reader.prepare_move(&[], &name, &["folder".into()], &target, || Ok(())).unwrap();
        let job = reader.submit_rename(&env.core, &plan, JobId(uuid::Uuid::new_v4())).unwrap();
        let r = env.wait(job.id); assert_eq!(r.state, JobState::Succeeded, "{r:?}");
        let (s, out) = sink();
        reader.decrypt_file(&["folder".into(), target], s, Default::default(), CryptomatorLengthPolicy::RequireTrusted(17), |_| Ok(())).unwrap();
        assert_eq!(out.borrow().bytes, b"cloud secret data");
    }
    let names: Vec<String> = reader.list_directory(&["folder".into()], 100, || Ok(())).unwrap().entries().iter().map(|e| e.name().to_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // Only ciphertext on the server: no plaintext names or contents.
    for path in walk_files(&env.cpath("Vault")) {
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(11).any(|w| w == b"cloud secre"), "plaintext content in {path:?}");
        assert!(!path.to_string_lossy().contains("short") && !path.to_string_lossy().contains("folder"), "plaintext name {path:?}");
    }
    // No staging folders left behind.
    let leftovers: Vec<String> = walk_all(&env.cpath("Vault")).into_iter().filter(|p| p.contains(".paranoic-")).collect();
    assert!(leftovers.is_empty(), "staging left in the vault: {leftovers:#?}");
}

fn walk_all(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() { let p = e.unwrap().path();
        let m = std::fs::symlink_metadata(&p).unwrap();
        out.push(format!("{} {} {}", p.display(), if m.is_dir() { "DIR" } else { "FILE" }, m.len()));
        if m.is_dir() { out.extend(walk_all(&p)); } }
    out
}

// ---------------- 3.9: recovery of an interrupted cloud rename ----------------
/// WebDAV provider that fails the final publication of a mapped rename once
/// armed (the wrapper is then left in the hidden staging folder).
struct Interrupting { inner: WebDav, armed: std::sync::atomic::AtomicBool }
impl CloudProvider for Interrupting {
    fn kind(&self) -> ProviderKind { self.inner.kind() }
    fn capabilities(&self) -> CloudCapabilities { self.inner.capabilities() }
    fn account_id(&self) -> paranoic_core::Result<String> { self.inner.account_id() }
    fn lookup(&self, parent: &CloudId, name: &str) -> paranoic_core::Result<Option<CloudEntry>> { self.inner.lookup(parent, name) }
    fn quota(&self) -> paranoic_core::Result<Quota> { self.inner.quota() }
    fn root(&self) -> paranoic_core::Result<CloudEntry> { self.inner.root() }
    fn metadata(&self, id: &CloudId) -> paranoic_core::Result<CloudEntry> { self.inner.metadata(id) }
    fn list(&self, folder: &CloudId, page: Option<&str>, limit: usize) -> paranoic_core::Result<CloudPage> { self.inner.list(folder, page, limit) }
    fn list_all(&self, folder: &CloudId, max: usize) -> paranoic_core::Result<Vec<CloudEntry>> { self.inner.list_all(folder, max) }
    fn create_folder(&self, parent: &CloudId, name: &str) -> paranoic_core::Result<CloudEntry> { self.inner.create_folder(parent, name) }
    fn open_download(&self, id: &CloudId, offset: u64) -> paranoic_core::Result<Box<dyn std::io::Read + Send>> { self.inner.open_download(id, offset) }
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn std::io::Read,
        progress: &mut dyn FnMut(u64) -> paranoic_core::Result<()>) -> paranoic_core::Result<CloudEntry> { self.inner.upload_new(parent, name, bytes, source, progress) }
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> paranoic_core::Result<CloudEntry> {
        let staging_step = new_name.is_some_and(|n| n == "entry" || n == "name.c9s" || n.starts_with("old-mapping-"));
        if expected.name == "entry" && !staging_step && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(paranoic_core::CoreError::new(paranoic_core::ErrorCode::Io, "simulated connection loss"));
        }
        self.inner.relocate(expected, new_parent, new_name)
    }
    fn trash(&self, expected: &CloudEntry) -> paranoic_core::Result<()> { self.inner.trash(expected) }
}

#[test] fn interrupted_cloud_rename_is_inspected_and_recovered_both_ways() {
    use paranoic_formats::*;
    let temp = Temp::new();
    let server = start(&temp, false);
    let provider = Arc::new(Interrupting { inner: dav(&server), armed: std::sync::atomic::AtomicBool::new(false) });
    let cloud_spool = temp.dir("cloud-spool"); private(&cloud_spool);
    let cloud = Arc::new(CloudStorage::open(StorageId(uuid::Uuid::new_v4()), provider.clone(), CloudId::parse("/").unwrap(), &cloud_spool).unwrap());
    let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.dir("spool"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new(); registry.register(cloud.clone()).unwrap(); registry.register(spool.clone()).unwrap();
    let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
    let wait = |id: JobId| { let until = Instant::now() + Duration::from_secs(60); loop {
        let j = core.job(id).unwrap(); if !matches!(j.state, JobState::Queued | JobState::Running) { return j; }
        assert!(Instant::now() < until); std::thread::sleep(Duration::from_millis(10)); } };
    let _kdf = KDF_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    let password = || zeroize::Zeroizing::new("recovery vault password".to_string());
    let job = CryptomatorVault::prepare_create(password(), || Ok(())).unwrap()
        .submit(&core, Location { storage: cloud.id(), path: ResourcePath::parse("Vault").unwrap() }, JobId(uuid::Uuid::new_v4())).unwrap();
    assert_eq!(wait(job.id).state, JobState::Succeeded);
    let reader = CryptomatorReader::open(cloud.clone(), ResourcePath::parse("Vault").unwrap(), password(), Duration::from_secs(300), || Ok(())).unwrap();
    let dir = reader.prepare_directory(&[], "folder", || Ok(())).unwrap();
    assert_eq!(wait(reader.submit_directory(&core, &dir, JobId(uuid::Uuid::new_v4())).unwrap().id).state, JobState::Succeeded);
    let import = |name: &str| {
        let (s, out) = sink();
        let prepared = reader.prepare_file_import(&[], name, &b"recover me"[..], s, Default::default(), |_| Ok(())).unwrap();
        std::fs::write(temp.0.join("spool/import"), &out.borrow().bytes).unwrap();
        let job = reader.submit_file_import(&core, &prepared, Location { storage: spool.id(), path: ResourcePath::parse("import").unwrap() }, JobId(uuid::Uuid::new_v4())).unwrap();
        assert_eq!(wait(job.id).state, JobState::Succeeded);
        std::fs::remove_file(temp.0.join("spool/import")).unwrap();
    };
    let read = |path: &[String]| { let (s, out) = sink();
        reader.decrypt_file(path, s, Default::default(), CryptomatorLengthPolicy::RequireTrusted(10), |_| Ok(())).map(|_| out.borrow().bytes.clone()) };
    for (round, action) in [MappedRecoveryAction::Complete, MappedRecoveryAction::RestoreSource].into_iter().enumerate() {
        let name = format!("{}{round}", "y".repeat(180)); // long => shortened wrapper => RenameMapped
        import(&name);
        let target = format!("{name}-moved");
        let plan = reader.prepare_move(&[], &name, &["folder".into()], &target, || Ok(())).unwrap();
        provider.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let job = reader.submit_rename(&core, &plan, JobId(uuid::Uuid::new_v4())).unwrap();
        let interrupted = wait(job.id);
        assert_ne!(interrupted.state, JobState::Succeeded, "interruption not simulated");
        // Neither name is readable now: the wrapper sits in staging.
        assert!(read(&[name.clone()]).is_err() && read(&["folder".into(), target.clone()]).is_err());
        let observation = core.inspect_mapped_rename(job.id, &AtomicBool::new(false)).unwrap();
        assert!(observation.staging_present, "{observation:?}");
        let recovery = core.recover_mapped_rename(job.id, JobId(uuid::Uuid::new_v4()), action).unwrap();
        let recovered = wait(recovery.id); assert_eq!(recovered.state, JobState::Succeeded, "{action:?}: {recovered:?}");
        match action {
            MappedRecoveryAction::Complete => { assert_eq!(read(&["folder".into(), target]).unwrap(), b"recover me"); assert!(read(&[name]).is_err()); }
            MappedRecoveryAction::RestoreSource => { assert_eq!(read(&[name]).unwrap(), b"recover me"); assert!(read(&["folder".into(), target]).is_err()); }
        }
    }
}
