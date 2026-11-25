//! У-5: 7z decoding runs only in the confined worker process; the app stages
//! and publishes. Same results as in-process for valid archives, and every
//! hostile case still fails closed with nothing left in the spool.
//! Interop cases need `7z` on PATH (PARANOIC_SKIP_INTEROP=1 skips them).
#![cfg(all(target_os = "linux", feature = "pdf"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::Cursor, path::{Path, PathBuf}, process::Command, sync::Arc};
use paranoic_core::*;
use paranoic_documents::*;
use paranoic_local::{Access, LocalStorage};
use zeroize::Zeroizing;

struct Temp(PathBuf);
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn temp() -> (Temp, Arc<LocalStorage>) {
    let p = std::env::temp_dir().join(format!("paranoic-7zw-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(p.join("spool")).unwrap();
    let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), p.join("spool"), Access::ReadWrite).unwrap());
    (Temp(p), spool)
}
fn worker() -> PathBuf { PathBuf::from(env!("CARGO_BIN_EXE_paranoic-pdf-worker")) }
fn rp(p: &str) -> ResourcePath { ResourcePath::parse(p).unwrap() }
fn ok(_: u64) -> Result<()> { Ok(()) }
fn ok0() -> Result<()> { Ok(()) }

fn sample() -> Vec<(&'static str, Option<Vec<u8>>)> {
    let big: Vec<u8> = (0..300_000u32).flat_map(|i| i.wrapping_mul(2654435761).to_le_bytes()).collect();
    vec![("docs", None), ("docs/readme.txt", Some(b"hello\r\nworld\n".to_vec())), ("empty", None),
        ("data/big.bin", Some(big)), ("data/zero.bin", Some(Vec::new()))]
}
fn ours() -> Vec<u8> {
    let sources = sample().into_iter().map(|(p, d)| match d {
        Some(d) => ZipSource { path: rp(p), bytes: d.len() as u64, reader: Some(Box::new(Cursor::new(d))) },
        None => ZipSource { path: rp(p), bytes: 0, reader: None },
    }).collect();
    let mut out = Cursor::new(Vec::new());
    SevenZipService::create(&mut out, sources, CodecLimits::default(), ok).unwrap();
    out.into_inner()
}
fn assert_tree(root: &Path) {
    for (p, d) in sample() {
        match d { Some(d) => assert_eq!(std::fs::read(root.join(p)).unwrap(), d, "{p}"), None => assert!(root.join(p).is_dir(), "{p}") }
    }
}
fn extract(spool: &LocalStorage, bytes: Vec<u8>, target: &str, password: Option<&str>, limits: CodecLimits) -> Result<PreparedExtraction> {
    let archive = IsolatedSevenZip::open(Cursor::new(bytes), limits, password.map(|p| Zeroizing::new(p.into())), &worker(), ok0)?;
    archive.prepare_extraction(spool, &rp(target), ok)
}
fn empty(dir: &Path) -> bool { std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true) }
fn interop() -> bool {
    if std::env::var_os("PARANOIC_SKIP_INTEROP").is_some() { interop_skip("7z interop (PARANOIC_SKIP_INTEROP)"); return false; }
    assert!(Command::new("7z").arg("-h").output().is_ok(), "7z missing; set PARANOIC_SKIP_INTEROP=1 to skip");
    true
}
fn make_7z(dir: &Path, name: &str, args: &[&str], files: &[(&str, &[u8])]) -> Vec<u8> {
    let src = dir.join(format!("{name}-src")); std::fs::create_dir_all(&src).unwrap();
    for (p, d) in files { let p = src.join(p); std::fs::create_dir_all(p.parent().unwrap()).unwrap(); std::fs::write(p, d).unwrap(); }
    let out = dir.join(format!("{name}.7z"));
    assert!(Command::new("7z").current_dir(&src).args(["a", "-bd", "-t7z"]).args(args).arg(&out).arg(".").output().unwrap().status.success());
    std::fs::read(out).unwrap()
}
fn fix_crc(b: &mut [u8]) {
    let off = u64::from_le_bytes(b[12..20].try_into().unwrap()) as usize;
    let size = u64::from_le_bytes(b[20..28].try_into().unwrap()) as usize;
    let crc = crc32fast::hash(&b[32 + off..32 + off + size]);
    b[28..32].copy_from_slice(&crc.to_le_bytes());
    let start = crc32fast::hash(&b[12..32]); b[8..12].copy_from_slice(&start.to_le_bytes());
}

#[test] fn isolated_preparation_accepts_more_than_128_files() {
    let (t, spool) = temp();
    let sources = (0..129).map(|i| ZipSource { path: rp(&format!("f{i:03}")), bytes: 1,
        reader: Some(Box::new(Cursor::new(vec![i as u8]))) }).collect();
    let mut out = Cursor::new(Vec::new());
    SevenZipService::create(&mut out, sources, CodecLimits::default(), ok).unwrap();
    let prepared = extract(spool.as_ref(), out.into_inner(), "many", None,
        CodecLimits { pending_files: 2, ..CodecLimits::default() }).unwrap();
    assert_eq!(prepared.files().len(), 129);
    assert_eq!(std::fs::read(t.0.join("spool/many/f128")).unwrap(), [128]);
}

#[test] fn worker_extracts_like_in_process() {
    let (t, spool) = temp();
    let prepared = extract(&spool, ours(), "ours", None, CodecLimits::default()).unwrap();
    assert_eq!(prepared.files().len(), 3);
    assert_tree(&t.0.join("spool/ours"));
    if !interop() { return; }
    let src = t.0.join("src");
    for (p, d) in sample() {
        let path = src.join(p);
        match d { Some(d) => { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(path, d).unwrap(); }
            None => std::fs::create_dir_all(path).unwrap() }
    }
    let names: Vec<&str> = sample().iter().map(|(p, _)| *p).collect();
    for (name, args, password) in [("lzma2", &["-m0=lzma2"][..], None), ("solid", &["-ms=on", "-mx=9"][..], None),
        ("aes", &["-pk3y"][..], Some("k3y")), ("aes_hdr", &["-pk3y", "-mhe=on"][..], Some("k3y"))] {
        let archive = t.0.join(format!("{name}.7z"));
        assert!(Command::new("7z").current_dir(&src).args(["a", "-bd", "-t7z"]).args(args).arg(&archive).args(&names).output().unwrap().status.success());
        let bytes = std::fs::read(&archive).unwrap();
        extract(&spool, bytes.clone(), name, password, CodecLimits::default()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_tree(&t.0.join("spool").join(name));
        if password.is_some() {
            let wrong = format!("{name}-wrong");
            assert!(extract(&spool, bytes, &wrong, Some("bad"), CodecLimits::default()).is_err(), "{name}: wrong password accepted");
            assert!(empty(&t.0.join("spool").join(&wrong)), "{name}: data left after wrong password");
        }
    }
}

#[test] fn worker_fails_closed() {
    let (t, spool) = temp();
    let good = ours();
    // Limits are enforced (in the worker and again in the app).
    let few = CodecLimits { archive: ArchiveLimits { entries: 2, ..ArchiveLimits::default() }, ..CodecLimits::default() };
    assert_eq!(extract(&spool, good.clone(), "few", None, few).unwrap_err().code, ErrorCode::LimitExceeded);
    for cut in [31usize, good.len() / 2, good.len() - 1] {
        assert!(extract(&spool, good[..cut].to_vec(), &format!("cut{cut}"), None, CodecLimits::default()).is_err(), "cut {cut}");
    }
    // Missing or crashing worker: unavailable, nothing staged.
    let r = IsolatedSevenZip::open(Cursor::new(good.clone()), CodecLimits::default(), None, Path::new("/bin/false"), ok0);
    assert_eq!(r.err().unwrap().code, ErrorCode::Unsupported);
    assert!(IsolatedSevenZip::open(Cursor::new(good.clone()), CodecLimits::default(), None, Path::new("relative"), ok0).is_err());
    if !interop() { return; }
    let dir = &t.0;
    // Corrupt stored payload: CRC fails in the worker, nothing published.
    let mut stored = make_7z(dir, "crc", &["-mx=0"], &[("good", b"0123456789"), ("zz", &[5u8; 1000])]);
    stored[32 + 500] ^= 0xff;
    assert!(extract(&spool, stored, "crc", None, CodecLimits::default()).is_err());
    assert!(empty(&dir.join("spool/crc")), "corrupt archive left files");
    // Lying unpacked size: the app stops the entry at the declared size.
    let mut lie = make_7z(dir, "lie", &["-mhc=off", "-mx=0"], &[("f", &[9u8; 200])]);
    let at = lie.windows(3).position(|w| w == [0x0c, 0x80, 0xc8]).unwrap();
    lie[at + 2] = 0xc4; fix_crc(&mut lie);
    assert!(extract(&spool, lie, "lie", None, CodecLimits::default()).is_err());
    assert!(empty(&dir.join("spool/lie")));
    // Oversized dictionary rejected before decoding.
    let mut dict = make_7z(dir, "dict", &["-mhc=off", "-m0=lzma2"], &[("f", &[7u8; 4096])]);
    let at = dict.windows(3).rposition(|w| w == [0x21, 0x21, 0x01]).unwrap() + 3;
    dict[at] = 40; fix_crc(&mut dict);
    assert!(extract(&spool, dict, "dict", None, CodecLimits::default()).is_err());
    // Ratio bomb bounded by declared sizes.
    let bomb = make_7z(dir, "bomb", &["-mx=9"], &[("zeros", &vec![0u8; 16 * 1024 * 1024])]);
    let small = CodecLimits { archive: ArchiveLimits { entry_bytes: 1024 * 1024, ..ArchiveLimits::default() }, ..CodecLimits::default() };
    assert_eq!(extract(&spool, bomb, "bomb", None, small).unwrap_err().code, ErrorCode::LimitExceeded);
    // Path traversal in a name.
    let mut evil = make_7z(dir, "trav", &["-mhc=off"], &[("ab_evil", b"pwned")]);
    let from: Vec<u8> = "ab_evil".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let to: Vec<u8> = "../evil".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let at = evil.windows(from.len()).position(|w| w == from.as_slice()).unwrap();
    evil[at..at + to.len()].copy_from_slice(&to); fix_crc(&mut evil);
    assert!(extract(&spool, evil, "trav", None, CodecLimits::default()).is_err());
    assert!(!dir.join("evil").exists() && !dir.join("spool/evil").exists());
}

/// A compromised worker: everything it says is checked by the application.
#[test] fn compromised_worker_is_contained() {
    use std::os::unix::fs::PermissionsExt;
    let (t, spool) = temp();
    let good = ours();
    let index = |name: &str, size: u64| -> String {
        let mut p = vec![0u8]; p.extend_from_slice(&size.to_le_bytes()); p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&(name.len() as u32).to_le_bytes()); p.extend_from_slice(name.as_bytes());
        let mut m = vec![2u8]; m.extend_from_slice(&(p.len() as u32).to_le_bytes()); m.extend(p);
        m.iter().map(|b| format!("\\{b:03o}")).collect()
    };
    let hex = |b: &[u8]| -> String { b.iter().map(|b| format!("\\{b:03o}")).collect() };
    let begin = hex(&[3, 4, 0, 0, 0, 0, 0, 0, 0]);
    let end_done = hex(&[5, 0, 0, 0, 0, 6, 0, 0, 0, 0]);
    let write = |name: &str, script: &str| -> PathBuf {
        let path = t.0.join(format!("fake-{name}.sh"));
        std::fs::write(&path, format!("#!/bin/sh\nprintf '{script}'\nsleep 5\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    };
    // Control: a well-behaved fake worker is accepted (the scripts are parsed as intended).
    let honest = write("honest", &format!("{}{begin}{}{end_done}", index("a", 1), hex(&[4, 1, 0, 0, 0, b'x'])));
    IsolatedSevenZip::open(Cursor::new(good.clone()), CodecLimits::default(), None, &honest, ok0).unwrap()
        .prepare_extraction(spool.as_ref(), &rp("honest"), ok).unwrap();
    assert_eq!(std::fs::read(t.0.join("spool/honest/a")).unwrap(), b"x");
    let cases: [(&str, String); 5] = [
        // More data than the index declared.
        ("oversize", format!("{}{begin}{}{end_done}", index("a", 5), hex(&[[4u8, 10, 0, 0, 0].as_slice(), b"XXXXXXXXXX"].concat()))),
        // Traversal in a name.
        ("traversal", index("../a", 1)),
        // Read request beyond the archive.
        ("read", hex(&[[1u8, 12, 0, 0, 0].as_slice(), &u64::MAX.to_le_bytes(), &16u32.to_le_bytes()].concat())),
        // Finished without delivering the entry.
        ("missing", format!("{}{end_done}", index("a", 1))),
        // Same entry twice.
        ("twice", format!("{}{begin}{}{begin}", index("a", 1), hex(&[4, 1, 0, 0, 0, b'x', 5, 0, 0, 0, 0]))),
    ];
    for (name, script) in cases {
        let path = write(name, &script);
        let result = IsolatedSevenZip::open(Cursor::new(good.clone()), CodecLimits::default(), None, &path, ok0)
            .and_then(|a| a.prepare_extraction(spool.as_ref(), &rp(name), ok));
        assert!(result.is_err(), "{name}: accepted");
        assert!(empty(&t.0.join("spool").join(name)), "{name}: left files in the spool");
    }
}

#[test] fn cancellation_interrupts_a_worker_that_sends_no_messages() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use std::os::unix::fs::PermissionsExt;
    let (t, _) = temp();
    let fake = t.0.join("waiting-worker");
    // Shell builtin only: no descendant process survives the killed worker.
    std::fs::write(&fake, "#!/bin/sh\nwhile read waiting; do :; done\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let signal = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(50)); flag.store(true, Ordering::Release); });
    let started = Instant::now();
    let result = IsolatedSevenZip::open_cancellable(Cursor::new(vec![0u8; 32]), CodecLimits::default(), None, &fake, cancelled, ok0);
    signal.join().unwrap();
    assert!(matches!(result, Err(CoreError { code: ErrorCode::Cancelled, .. })));
    assert!(started.elapsed() < Duration::from_secs(2), "cancellation waited for the 60-second IPC deadline");
}

/// E2.1 through the confined worker: encrypted flag in the index and the two
/// password outcomes survive the process boundary; nothing stays in the spool.
#[test] fn worker_reports_password_required_and_wrong() {
    if !interop() { return; }
    let (t, spool) = temp();
    let files: &[(&str, &[u8])] = &[("a.txt", b"secret text"), ("dir/b.bin", &[7u8; 4096])];
    let open = |bytes: Vec<u8>, password: Option<&str>| IsolatedSevenZip::open(Cursor::new(bytes), CodecLimits::default(),
        password.map(|p| Zeroizing::new(p.into())), &worker(), ok0);
    let data = make_7z(&t.0, "data", &["-pk3y"], files);
    let listed = open(data.clone(), None).unwrap();
    assert!(any_encrypted(listed.entries()) && listed.entries().iter().all(|e| e.encrypted != e.directory));
    let err = listed.prepare_extraction(spool.as_ref(), &rp("nopw"), ok).unwrap_err();
    assert!(is_password_required(&err), "{err:?}");
    let err = extract(&spool, data.clone(), "bad", Some("bad"), CodecLimits::default()).unwrap_err();
    assert!(is_wrong_password(&err), "{err:?}");
    extract(&spool, data, "good", Some("k3y"), CodecLimits::default()).unwrap();
    let names = make_7z(&t.0, "names", &["-pk3y", "-mhe=on"], files);
    assert!(is_password_required(&open(names.clone(), None).err().unwrap()));
    assert!(is_wrong_password(&open(names, Some("bad")).err().unwrap()));
    assert!(!any_encrypted(open(ours(), None).unwrap().entries()));
    for leftover in ["nopw", "bad"] { assert!(empty(&t.0.join("spool").join(leftover)), "{leftover}"); }
}

/// Audit 18: with PARANOIC_REQUIRE_INTEROP set, a missing fixture or tool fails
/// the run instead of passing as a silent skip.
#[allow(dead_code)]
fn interop_skip(what: &str) {
    assert!(std::env::var_os("PARANOIC_REQUIRE_INTEROP").is_none(), "{what} required by PARANOIC_REQUIRE_INTEROP");
    eprintln!("{what}: skipped");
}
