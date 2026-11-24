//! Block 2 verification: hostile ZIP/7z archives, 7-Zip / Info-ZIP interop,
//! text documents and extraction as a journaled Core job.
//! Interop tests require `7z`, `zip` and `unzip` on PATH; set
//! PARANOIC_SKIP_INTEROP=1 to skip them explicitly.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::{Cursor, Read, Write}, path::{Path, PathBuf}, process::Command, sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant}, cell::RefCell, rc::Rc};
use paranoic_core::*;
use paranoic_documents::*;
use paranoic_local::{Access, LocalStorage};
use zeroize::Zeroizing;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-b2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap(); Self(p)
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

struct H { temp: Temp, files: Arc<LocalStorage>, spool: Arc<LocalStorage>, core: Core }
impl H {
    fn new() -> Self {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("files")).unwrap(); std::fs::create_dir(temp.0.join("spool")).unwrap();
        let files = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.0.join("files"), Access::ReadWrite).unwrap());
        let spool = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), temp.0.join("spool"), Access::ReadWrite).unwrap());
        let mut registry = StorageRegistry::new();
        registry.register(files.clone()).unwrap(); registry.register(spool.clone()).unwrap();
        let core = Core::open(temp.0.join("state"), CoreConfig::default(), registry).unwrap();
        Self { temp, files, spool, core }
    }
    fn fpath(&self, rel: &str) -> PathBuf { self.temp.0.join("files").join(rel) }
    fn loc(&self, storage: &LocalStorage, p: &str) -> Location { Location { storage: storage.id(), path: rp(p) } }
    fn wait(&self, id: JobId) -> JobRecord {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let job = self.core.job(id).unwrap();
            if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
            assert!(Instant::now() < until, "job timed out"); std::thread::sleep(Duration::from_millis(5));
        }
    }
}
fn rp(p: &str) -> ResourcePath { ResourcePath::parse(p).unwrap() }
fn ok(_: u64) -> paranoic_core::Result<()> { Ok(()) }
fn ok0() -> paranoic_core::Result<()> { Ok(()) }
fn pw(s: &str) -> Option<Zeroizing<String>> { Some(Zeroizing::new(s.into())) }
fn code<T>(r: paranoic_core::Result<T>) -> ErrorCode { match r { Err(e) => e.code, Ok(_) => panic!("expected error") } }

fn interop() -> bool {
    if std::env::var_os("PARANOIC_SKIP_INTEROP").is_some() { eprintln!("interop skipped by PARANOIC_SKIP_INTEROP"); return false; }
    for tool in ["7z", "zip", "unzip"] {
        assert!(Command::new(tool).arg("-h").output().is_ok(), "{tool} missing; set PARANOIC_SKIP_INTEROP=1 to skip");
    }
    true
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.env("LC_ALL", "C.UTF-8").output().unwrap();
    assert!(out.status.success(), "{:?} failed: {}\n{}", cmd, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn fails(cmd: &mut Command) { assert!(!cmd.env("LC_ALL", "C.UTF-8").output().unwrap().status.success(), "{cmd:?} unexpectedly succeeded"); }

fn sample() -> Vec<(&'static str, Option<Vec<u8>>)> {
    let big: Vec<u8> = (0..300_000u32).flat_map(|i| (i.wrapping_mul(2654435761)).to_le_bytes()).collect();
    vec![("docs", None), ("docs/readme.txt", Some(b"hello\r\nworld\n".to_vec())),
        ("docs/unicode-\u{043f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}.md", Some("# \u{0417}\u{0430}\u{0433}\n".as_bytes().to_vec())),
        ("empty", None), ("data/big.bin", Some(big)), ("data/zero.bin", Some(Vec::new()))]
}
fn sources() -> Vec<ZipSource> {
    sample().into_iter().map(|(p, d)| match d {
        Some(d) => ZipSource { path: rp(p), bytes: d.len() as u64, reader: Some(Box::new(Cursor::new(d))) },
        None => ZipSource { path: rp(p), bytes: 0, reader: None },
    }).collect()
}
fn assert_tree(root: &Path) {
    for (p, d) in sample() {
        let path = root.join(p);
        match d { Some(d) => assert_eq!(std::fs::read(&path).unwrap(), d, "{p}"), None => assert!(path.is_dir(), "{p}") }
    }
}
fn write_tree(root: &Path) {
    for (p, d) in sample() {
        let path = root.join(p);
        match d { Some(d) => { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(path, d).unwrap(); }
            None => std::fs::create_dir_all(path).unwrap() }
    }
}
fn our_zip(password: Option<&str>) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    ZipService::create(&mut out, sources(), ArchiveLimits::default(), password.map(|p| Zeroizing::new(p.into())), ok).unwrap();
    out.into_inner()
}
fn our_7z() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    SevenZipService::create(&mut out, sources(), CodecLimits::default(), ok).unwrap();
    out.into_inner()
}
fn zip_extract(h: &H, bytes: Vec<u8>, target: &str, password: Option<&str>) -> paranoic_core::Result<u64> {
    let mut service = ZipService::open(Cursor::new(bytes), ArchiveLimits::default(), ok0)?;
    service.extract(h.files.as_ref(), &rp(target), password.map(|p| Zeroizing::new(p.into())), ok)
}
fn zip_open(bytes: Vec<u8>) -> paranoic_core::Result<ZipService<Cursor<Vec<u8>>>> {
    ZipService::open(Cursor::new(bytes), ArchiveLimits::default(), ok0)
}
fn sz_open(bytes: Vec<u8>, password: Option<&str>) -> paranoic_core::Result<SevenZipService<Cursor<Vec<u8>>>> {
    SevenZipService::open(Cursor::new(bytes), CodecLimits::default(), password.map(|p| Zeroizing::new(p.into())), ok0)
}

// ---------- raw ZIP builder for hostile layouts ----------
struct Raw { name: Vec<u8>, data: Vec<u8>, crc: Option<u32>, usize_cd: Option<u32>, attr: u32 }
fn raw(name: &str, data: &[u8]) -> Raw { Raw { name: name.as_bytes().to_vec(), data: data.to_vec(), crc: None, usize_cd: None, attr: 0o100644 << 16 } }
fn craft(entries: &[Raw]) -> Vec<u8> {
    let mut out = Vec::new(); let mut cd = Vec::new();
    for e in entries {
        let crc = e.crc.unwrap_or_else(|| crc32fast::hash(&e.data));
        let offset = out.len() as u32; let size = e.data.len() as u32;
        let usize_cd = e.usize_cd.unwrap_or(size);
        out.extend_from_slice(&0x04034b50u32.to_le_bytes()); out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes()); out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes()); out.extend_from_slice(&size.to_le_bytes()); out.extend_from_slice(&usize_cd.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes()); out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.name); out.extend_from_slice(&e.data);
        cd.extend_from_slice(&0x02014b50u32.to_le_bytes()); cd.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes()); cd.extend_from_slice(&0x0800u16.to_le_bytes()); cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes()); cd.extend_from_slice(&0x21u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes()); cd.extend_from_slice(&size.to_le_bytes()); cd.extend_from_slice(&usize_cd.to_le_bytes());
        cd.extend_from_slice(&(e.name.len() as u16).to_le_bytes()); cd.extend_from_slice(&[0u8; 8]);
        cd.extend_from_slice(&e.attr.to_le_bytes()); cd.extend_from_slice(&offset.to_le_bytes()); cd.extend_from_slice(&e.name);
    }
    let cd_offset = out.len() as u32; out.extend_from_slice(&cd);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes()); out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes()); out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes()); out.extend_from_slice(&cd_offset.to_le_bytes()); out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test] fn zip_raw_builder_is_valid() -> TestResult {
    let bytes = craft(&[raw("a.txt", b"abc"), raw("d/b.txt", b"x")]);
    let h = H::new(); zip_extract(&h, bytes.clone(), "out", None)?;
    assert_eq!(std::fs::read(h.fpath("out/a.txt"))?, b"abc");
    if interop() {
        let p = h.temp.0.join("raw.zip"); std::fs::write(&p, &bytes)?;
        run(Command::new("unzip").arg("-tq").arg(&p));
    }
    Ok(())
}

#[test] fn zip_hostile_names_are_rejected_before_any_write() {
    let cases: Vec<Vec<Raw>> = vec![
        vec![raw("../evil", b"x")], vec![raw("a/../../evil", b"x")], vec![raw("/abs", b"x")],
        vec![raw("..\\evil", b"x")], vec![raw("C:/evil", b"x")], vec![raw("c:evil", b"x")],
        vec![raw("a/./b", b"x")], vec![raw("a//b", b"x")], vec![raw("nul\0x", b"x")], vec![raw("CON.txt", b"x")],
        vec![raw(".paranoic-x", b"x")], vec![raw("dot.", b"x")],
        vec![raw("dup", b"1"), raw("dup", b"2")], vec![raw("Case", b"1"), raw("case", b"2")],
        vec![raw("f", b"1"), raw("f/child", b"2")], vec![raw("f/child", b"2"), raw("f", b"1")],
        vec![Raw { attr: 0o120777 << 16, ..raw("link", b"/etc/passwd") }],
        vec![Raw { attr: 0o020644 << 16, ..raw("chardev", b"") }],
        vec![Raw { name: vec![0xff, 0xfe, b'a'], ..raw("x", b"x") }],
    ];
    for (i, case) in cases.iter().enumerate() {
        let h = H::new();
        let r = zip_extract(&h, craft(case), "out", None);
        assert!(r.is_err(), "case {i} accepted");
        assert!(!h.fpath("out").exists(), "case {i} created destination");
        assert!(!h.temp.0.join("evil").exists() && !Path::new("/abs").exists());
    }
}

#[test] fn zip_lying_sizes_crc_and_truncation() -> TestResult {
    // Central directory declares 3 bytes; payload holds 5 (stored): output overrun.
    let h = H::new();
    let r = zip_extract(&h, craft(&[Raw { usize_cd: Some(3), ..raw("a", b"12345") }]), "o1", None);
    assert!(r.is_err());
    assert!(!h.fpath("o1/a").exists(), "oversized entry published");
    // CRC mismatch: file must not be published; earlier files remain (NeedsAttention).
    let r = zip_extract(&h, craft(&[raw("good", b"ok"), Raw { crc: Some(1), ..raw("bad", b"payload") }]), "o2", None);
    assert_eq!(code(r), ErrorCode::NeedsAttention);
    assert_eq!(std::fs::read(h.fpath("o2/good"))?, b"ok");
    assert!(!h.fpath("o2/bad").exists(), "corrupt entry published");
    // Truncated archives, garbage and EOCD claims of multi-disk.
    let good = our_zip(None);
    for cut in [10usize, good.len() / 2, good.len() - 1] { assert!(zip_open(good[..cut].to_vec()).is_err(), "cut {cut}"); }
    assert!(zip_open(vec![0u8; 100]).is_err());
    let mut split = craft(&[raw("a", b"1")]); let n = split.len(); split[n - 18] = 1; // disk number
    assert!(zip_open(split).is_err());
    // Central directory offset pointing beyond its boundary.
    let mut off = craft(&[raw("a", b"1")]); let n = off.len(); off[n - 6..n - 2].copy_from_slice(&0xfff0u32.to_le_bytes());
    assert!(zip_open(off).is_err());
    // Corrupt deflate stream in our own archive: reported, never silently accepted.
    let mut flipped = our_zip(None); flipped[200] ^= 0xff;
    let h2 = H::new();
    if let Ok(mut s) = zip_open(flipped) { assert!(s.extract(h2.files.as_ref(), &rp("o"), None, ok).is_err()); }
    Ok(())
}

#[test] fn zip_bomb_and_limits() -> TestResult {
    let zeros = vec![0u8; 8 * 1024 * 1024];
    let mut out = Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(&mut out);
    w.start_file("bomb", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))?;
    w.write_all(&zeros)?; w.finish()?;
    let bomb = out.into_inner();
    let strict = ArchiveLimits { ratio: 100, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(bomb.clone()), strict, ok0)), ErrorCode::LimitExceeded);
    let small = ArchiveLimits { entry_bytes: 1024 * 1024, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(bomb.clone()), small, ok0)), ErrorCode::LimitExceeded);
    let total = ArchiveLimits { total_bytes: 1024, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(bomb.clone()), total, ok0)), ErrorCode::LimitExceeded);
    let tiny = ArchiveLimits { archive_bytes: 100, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(bomb), tiny, ok0)), ErrorCode::LimitExceeded);
    let many: Vec<Raw> = (0..20).map(|i| raw(&format!("f{i}"), b"x")).collect();
    let few = ArchiveLimits { entries: 10, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(craft(&many)), few, ok0)), ErrorCode::LimitExceeded);
    let deep = (0..70).map(|_| "d").collect::<Vec<_>>().join("/");
    assert!(zip_open(craft(&[raw(&deep, b"x")])).is_err());
    let cd = ArchiveLimits { directory_bytes: 64, ..ArchiveLimits::default() };
    assert_eq!(code(ZipService::open(Cursor::new(craft(&many)), cd, ok0)), ErrorCode::LimitExceeded);
    assert!(ZipService::open(Cursor::new(craft(&many)), ArchiveLimits { entries: 0, ..ArchiveLimits::default() }, ok0).is_err());
    // Creation output limit.
    let mut out = Cursor::new(Vec::new());
    assert!(ZipService::create(&mut out, sources(), ArchiveLimits { archive_bytes: 1000, ..ArchiveLimits::default() }, None, ok).is_err());
    Ok(())
}

#[test] fn zip_roundtrip_aes_wrong_password_tamper_and_existing_target() -> TestResult {
    let h = H::new();
    zip_extract(&h, our_zip(None), "plain", None)?; assert_tree(&h.fpath("plain"));
    let aes = our_zip(Some("correct horse"));
    zip_extract(&h, aes.clone(), "aes", Some("correct horse"))?; assert_tree(&h.fpath("aes"));
    assert!(zip_extract(&h, aes.clone(), "wrong", Some("battery staple")).is_err());
    assert!(!h.fpath("wrong/docs/readme.txt").exists());
    assert!(zip_extract(&h, aes.clone(), "nopw", None).is_err());
    // Flip a byte inside the encrypted payload of the big file: HMAC must fail.
    let mut tampered = aes.clone(); let i = tampered.len() / 2; tampered[i] ^= 0x01;
    let r = zip_extract(&h, tampered, "tampered", Some("correct horse"));
    assert!(r.is_err()); assert!(!h.fpath("tampered/data/big.bin").exists());
    // Existing destination is never merged.
    assert!(zip_extract(&h, our_zip(None), "plain", None).is_err());
    // Cancellation before first write.
    let mut s = zip_open(our_zip(None))?;
    assert_eq!(code(s.extract(h.files.as_ref(), &rp("cancel"), None, |_| Err(CoreError::new(ErrorCode::Cancelled, "x")))), ErrorCode::Cancelled);
    assert!(!h.fpath("cancel").exists());
    // Creation rejects bad requests.
    let mut out = Cursor::new(Vec::new());
    let dup = vec![ZipSource { path: rp("a"), bytes: 0, reader: None }, ZipSource { path: rp("A"), bytes: 0, reader: None }];
    assert!(ZipService::create(&mut out, dup, ArchiveLimits::default(), None, ok).is_err());
    let lying = vec![ZipSource { path: rp("a"), bytes: 10, reader: Some(Box::new(Cursor::new(vec![1u8; 3]))) }];
    assert_eq!(code(ZipService::create(&mut Cursor::new(Vec::new()), lying, ArchiveLimits::default(), None, ok)), ErrorCode::SourceChanged);
    assert!(ZipService::create(&mut Cursor::new(Vec::new()), sources(), ArchiveLimits::default(), pw(""), ok).is_err());
    Ok(())
}

#[test] fn zip_interop_with_info_zip_and_7zip() -> TestResult {
    if !interop() { return Ok(()); }
    let h = H::new(); let dir = &h.temp.0;
    // Ours -> Info-ZIP and 7-Zip.
    let ours = dir.join("ours.zip"); std::fs::write(&ours, our_zip(None))?;
    run(Command::new("unzip").arg("-tq").arg(&ours));
    run(Command::new("unzip").arg("-q").arg(&ours).arg("-d").arg(dir.join("unzip_out"))); assert_tree(&dir.join("unzip_out"));
    run(Command::new("7z").arg("t").arg(&ours));
    let aes = dir.join("aes.zip"); std::fs::write(&aes, our_zip(Some("pw-123")))?;
    run(Command::new("7z").arg("x").arg("-ppw-123").arg(format!("-o{}", dir.join("7z_aes").display())).arg(&aes));
    assert_tree(&dir.join("7z_aes"));
    fails(Command::new("7z").arg("t").arg("-pwrong").arg(&aes));
    // Info-ZIP / 7-Zip -> ours (Deflate, Store, forced ZIP64, ZipCrypto rejected, AES-256 accepted).
    let src = dir.join("src"); std::fs::create_dir(&src)?; write_tree(&src);
    let names: Vec<&str> = sample().iter().map(|(p, _)| *p).collect();
    let mut i = 0;
    let mut check = |zip: &Path, password: Option<&str>| -> TestResult {
        i += 1; let target = format!("in{i}");
        zip_extract(&h, std::fs::read(zip)?, &target, password)?; assert_tree(&h.fpath(&target)); Ok(())
    };
    let z1 = dir.join("infozip.zip"); run(Command::new("zip").current_dir(&src).arg("-qr").arg(&z1).args(&names)); check(&z1, None)?;
    let z2 = dir.join("infozip64.zip"); run(Command::new("zip").current_dir(&src).arg("-qr").arg("-fz").arg(&z2).args(&names)); check(&z2, None)?;
    let z3 = dir.join("store.zip"); run(Command::new("zip").current_dir(&src).arg("-qr0").arg(&z3).args(&names)); check(&z3, None)?;
    let z4 = dir.join("7z.zip"); run(Command::new("7z").current_dir(&src).args(["a", "-tzip", "-bd"]).arg(&z4).args(&names)); check(&z4, None)?;
    let z5 = dir.join("7zaes.zip"); run(Command::new("7z").current_dir(&src).args(["a", "-tzip", "-bd", "-mem=AES256", "-psecret"]).arg(&z5).args(&names));
    check(&z5, Some("secret"))?;
    assert!(zip_extract(&h, std::fs::read(&z5)?, "z5wrong", Some("nope")).is_err());
    // Info-ZIP symlink entries (-y) are refused.
    std::os::unix::fs::symlink("/etc/passwd", src.join("link"))?;
    let z6 = dir.join("symlink.zip"); run(Command::new("zip").current_dir(&src).arg("-qy").arg(&z6).arg("link"));
    assert!(zip_open(std::fs::read(&z6)?).is_err());
    Ok(())
}

// ---------- 7z ----------
fn sz_extract(h: &H, bytes: Vec<u8>, target: &str, password: Option<&str>) -> paranoic_core::Result<u64> {
    sz_open(bytes, password)?.extract(h.files.as_ref(), &rp(target), ok)
}
/// Rewrite both 7z CRCs after patching an unencoded header (-mhc=off).
fn fix_7z_crc(b: &mut [u8]) {
    let off = u64::from_le_bytes(b[12..20].try_into().unwrap()) as usize;
    let size = u64::from_le_bytes(b[20..28].try_into().unwrap()) as usize;
    let crc = crc32fast::hash(&b[32 + off..32 + off + size]);
    b[28..32].copy_from_slice(&crc.to_le_bytes());
    let start = crc32fast::hash(&b[12..32]); b[8..12].copy_from_slice(&start.to_le_bytes());
}
fn utf16(s: &str) -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() }
fn replace(b: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let i = b.windows(from.len()).position(|w| w == from).expect("pattern"); b[i..i + to.len()].copy_from_slice(to);
}
fn make_7z(dir: &Path, name: &str, args: &[&str], files: &[(&str, &[u8])]) -> Vec<u8> {
    let src = dir.join(format!("{name}-src")); std::fs::create_dir_all(&src).unwrap();
    for (p, d) in files { let p = src.join(p); std::fs::create_dir_all(p.parent().unwrap()).unwrap(); std::fs::write(p, d).unwrap(); }
    let out = dir.join(format!("{name}.7z"));
    run(Command::new("7z").current_dir(&src).args(["a", "-bd", "-t7z"]).args(args).arg(&out).arg("."));
    std::fs::read(out).unwrap()
}

#[test] fn sevenz_roundtrip_and_interop() -> TestResult {
    let h = H::new();
    sz_extract(&h, our_7z(), "ours", None)?; assert_tree(&h.fpath("ours"));
    if !interop() { return Ok(()); }
    let dir = &h.temp.0;
    let ours = dir.join("ours.7z"); std::fs::write(&ours, our_7z())?;
    run(Command::new("7z").arg("t").arg(&ours));
    let listing = run(Command::new("7z").arg("l").arg("-slt").arg(&ours));
    assert!(!listing.contains("Anti = +"), "directories written as 7z anti-items");
    run(Command::new("7z").arg("x").arg("-bd").arg(format!("-o{}", dir.join("x").display())).arg(&ours)); assert_tree(&dir.join("x"));
    let short = run(Command::new("7z").arg("l").arg(&ours));
    assert!(short.lines().any(|l| l.contains("D....") && l.trim_end().ends_with(" docs")), "directory flag lost");
    let src = dir.join("src"); std::fs::create_dir(&src)?; write_tree(&src);
    let names: Vec<&str> = sample().iter().map(|(p, _)| *p).collect();
    let variants: [(&str, &[&str], Option<&str>); 6] = [
        ("lzma2", &["-m0=lzma2"], None), ("solid", &["-ms=on", "-mx=9"], None), ("copy", &["-mx=0"], None),
        ("lzma", &["-m0=lzma"], None), ("aes", &["-pk3y"], Some("k3y")), ("aes_hdr", &["-pk3y", "-mhe=on"], Some("k3y")),
    ];
    for (name, args, password) in variants {
        let archive = dir.join(format!("{name}.7z"));
        run(Command::new("7z").current_dir(&src).args(["a", "-bd", "-t7z"]).args(args).arg(&archive).args(&names));
        sz_extract(&h, std::fs::read(&archive)?, name, password).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_tree(&h.fpath(name));
        if password.is_some() {
            let wrong = format!("{name}_wrong");
            let r = sz_open(std::fs::read(&archive)?, Some("bad")).and_then(|mut s| s.extract(h.files.as_ref(), &rp(&wrong), ok));
            assert!(r.is_err(), "{name} wrong password accepted");
            assert!(!h.fpath(&format!("{wrong}/data/big.bin")).exists());
        }
    }
    Ok(())
}

#[test] fn sevenz_hostile_inputs() -> TestResult {
    let h = H::new(); let dir = &h.temp.0;
    // Truncation and garbage.
    let good = our_7z();
    for cut in [0usize, 31, good.len() / 2, good.len() - 1] { assert!(sz_open(good[..cut].to_vec(), None).is_err(), "cut {cut}"); }
    let mut bad_crc = good.clone(); bad_crc[40] ^= 0xff;
    if let Ok(mut s) = sz_open(bad_crc, None) {
        assert!(s.extract(h.files.as_ref(), &rp("badcrc"), ok).is_err());
        assert!(!h.fpath("badcrc/data/big.bin").exists());
    }
    // Header pointing past EOF / huge header size.
    let mut huge = good.clone(); huge[20..28].copy_from_slice(&(1u64 << 40).to_le_bytes());
    assert!(sz_open(huge, None).is_err());
    // Entry and size limits.
    let few = CodecLimits { archive: ArchiveLimits { entries: 2, ..ArchiveLimits::default() }, ..CodecLimits::default() };
    assert_eq!(code(SevenZipService::open(Cursor::new(good.clone()), few, None, ok0)), ErrorCode::LimitExceeded);
    let pending = CodecLimits { pending_files: 2, ..CodecLimits::default() };
    let mut s = SevenZipService::open(Cursor::new(good.clone()), pending, None, ok0)?;
    assert!(s.extract(h.files.as_ref(), &rp("pending"), ok).is_err());
    assert!(!h.fpath("pending").exists());
    if !interop() { return Ok(()); }
    // Path traversal: patch a same-length name in an unencoded header.
    let mut evil = make_7z(dir, "trav", &["-mhc=off"], &[("ab_evil", b"pwned")]);
    replace(&mut evil, &utf16("ab_evil"), &utf16("../evil")); fix_7z_crc(&mut evil);
    assert!(sz_open(evil, None).is_err()); assert!(!dir.join("evil").exists());
    let mut abs = make_7z(dir, "abs", &["-mhc=off"], &[("xetc", b"pwned")]);
    replace(&mut abs, &utf16("xetc"), &utf16("/etc")); fix_7z_crc(&mut abs);
    assert!(sz_open(abs, None).is_err());
    let mut back = make_7z(dir, "back", &["-mhc=off"], &[("a_b", b"x")]);
    replace(&mut back, &utf16("a_b"), &utf16("a\\b")); fix_7z_crc(&mut back);
    assert!(sz_open(back, None).is_err());
    let mut dup = make_7z(dir, "dup", &["-mhc=off"], &[("aa", b"1"), ("ab", b"2")]);
    replace(&mut dup, &utf16("ab"), &utf16("AA")); fix_7z_crc(&mut dup);
    assert!(sz_open(dup, None).is_err());
    // Symlink stored by 7-Zip (-snl).
    let src = dir.join("sl"); std::fs::create_dir(&src)?;
    std::os::unix::fs::symlink("/etc/passwd", src.join("link"))?;
    let sl = dir.join("sl.7z"); run(Command::new("7z").current_dir(&src).args(["a", "-bd", "-snl"]).arg(&sl).arg("link"));
    assert!(sz_open(std::fs::read(&sl)?, None).is_err(), "symlink accepted");
    // Oversized LZMA2 dictionary property (40 = 4 GiB) rejected before decoding.
    let mut dict = make_7z(dir, "dict", &["-mhc=off", "-m0=lzma2"], &[("f", &[7u8; 4096])]);
    let at = dict.windows(3).rposition(|w| w == [0x21, 0x21, 0x01]).expect("LZMA2 coder") + 3;
    dict[at] = 40; fix_7z_crc(&mut dict);
    assert!(sz_open(dict, None).is_err());
    // Lying unpacked size in the header: actual output differs from declared.
    let mut lie = make_7z(dir, "lie", &["-mhc=off", "-mx=0"], &[("f", &[9u8; 200])]);
    replace(&mut lie, &[0x0c, 0x80, 0xc8], &[0x0c, 0x80, 0xc4]); fix_7z_crc(&mut lie);
    let r = sz_open(lie, None).and_then(|mut s| s.extract(h.files.as_ref(), &rp("lie"), ok));
    assert!(r.is_err()); assert!(!h.fpath("lie/f").exists());
    // Payload corruption in a stored (copy) 7z: header intact, CRC must fail
    // and the corrupt file must not be published.
    let mut stored = make_7z(dir, "crc", &["-mx=0"], &[("good", b"0123456789"), ("zz", &[5u8; 1000])]);
    let pos = 32 + 500; stored[pos] ^= 0xff;
    let mut s = sz_open(stored, None).expect("header must remain readable");
    assert!(s.extract(h.files.as_ref(), &rp("crc"), ok).is_err());
    assert!(!h.fpath("crc/zz").exists() && !h.fpath("crc/good").exists(), "7z publishes nothing on failure");
    // Ratio bomb is bounded by declared-size limits.
    let bomb = make_7z(dir, "bomb", &["-mx=9"], &[("zeros", &vec![0u8; 16 * 1024 * 1024])]);
    let small = CodecLimits { archive: ArchiveLimits { entry_bytes: 1024 * 1024, ..ArchiveLimits::default() }, ..CodecLimits::default() };
    assert_eq!(code(SevenZipService::open(Cursor::new(bomb), small, None, ok0)), ErrorCode::LimitExceeded);
    Ok(())
}

// ---------- extraction as a Core job ----------
#[test] fn extraction_is_a_journaled_core_job() -> TestResult {
    let h = H::new();
    let mut zip = zip_open(our_zip(Some("pw")))?;
    let prepared = zip.prepare_extraction(h.spool.as_ref(), &rp("job1"), pw("pw"), ok)?;
    assert_eq!(prepared.files().len(), 4);
    let job = JobId(uuid::Uuid::new_v4());
    prepared.submit_new(&h.core, h.loc(&h.spool, "job1"), h.loc(&h.files, "Extracted"), job)?;
    let done = h.wait(job);
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert_tree(&h.fpath("Extracted"));
    assert!(!h.temp.0.join("spool/job1").exists(), "spool retained after success");
    prepared.verify_published(&h.core, &h.loc(&h.files, "Extracted"), &AtomicBool::new(false))?;
    // Journal survives reopen and still describes the job.
    assert!(matches!(h.core.job(job)?.operation, Operation::MoveDirectoryExpected { .. }));
    // Tamper after publication is detected.
    std::fs::write(h.fpath("Extracted/docs/readme.txt"), b"changed")?;
    assert_eq!(code(prepared.verify_published(&h.core, &h.loc(&h.files, "Extracted"), &AtomicBool::new(false))), ErrorCode::Integrity);
    // Existing destination: job fails without merging, spool retained for retry.
    let mut sz = sz_open(our_7z(), None)?;
    let p2 = sz.prepare_extraction(h.spool.as_ref(), &rp("job2"), ok)?;
    let job2 = JobId(uuid::Uuid::new_v4());
    p2.submit_new(&h.core, h.loc(&h.spool, "job2"), h.loc(&h.files, "Extracted"), job2)?;
    assert_ne!(h.wait(job2).state, JobState::Succeeded);
    assert!(h.temp.0.join("spool/job2/data/big.bin").exists(), "spool lost on failed publication");
    let job3 = JobId(uuid::Uuid::new_v4());
    p2.submit_new(&h.core, h.loc(&h.spool, "job2"), h.loc(&h.files, "Other"), job3)?;
    assert_eq!(h.wait(job3).state, JobState::Succeeded);
    p2.verify_published(&h.core, &h.loc(&h.files, "Other"), &AtomicBool::new(false))?;
    // Failed preparation discards its own spool; an existing spool is never reused.
    let mut corrupt = craft(&[raw("good", b"ok"), Raw { crc: Some(1), ..raw("bad", b"payload") }]);
    let mut z = zip_open(std::mem::take(&mut corrupt))?;
    assert!(z.prepare_extraction(h.spool.as_ref(), &rp("job4"), None, ok).is_err());
    assert!(!h.temp.0.join("spool/job4").exists(), "failed spool retained");
    std::fs::create_dir(h.temp.0.join("spool/busy"))?;
    assert_eq!(code(zip_open(our_zip(None))?.prepare_extraction(h.spool.as_ref(), &rp("busy"), None, ok)), ErrorCode::AlreadyExists);
    Ok(())
}

// ---------- text ----------
struct Mem(Rc<RefCell<(Vec<u8>, bool)>>, Vec<u8>);
impl Write for Mem { fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.1.extend_from_slice(b); Ok(b.len()) } fn flush(&mut self) -> std::io::Result<()> { Ok(()) } }
impl paranoic_crypto::StreamSink for Mem {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { let mut o = self.0.borrow_mut(); o.0 = self.1.clone(); o.1 = true; Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}
fn save(doc: &TextDocument) -> Vec<u8> {
    let out = Rc::new(RefCell::new((Vec::new(), false)));
    doc.save(Box::new(Mem(out.clone(), Vec::new())), ok0).unwrap();
    assert!(out.borrow().1); let v = out.borrow().0.clone(); v
}
#[test] fn text_encodings_edits_and_limits() -> TestResult {
    let cases: Vec<(Vec<u8>, TextEncoding)> = vec![
        (b"plain\r\nline\n".to_vec(), TextEncoding::Utf8),
        ([&[0xef, 0xbb, 0xbf][..], "\u{041f}\u{0440}\u{0438}".as_bytes()].concat(), TextEncoding::Utf8Bom),
        ([vec![0xff, 0xfe], "a\u{1f600}b".encode_utf16().flat_map(|u| u.to_le_bytes()).collect()].concat(), TextEncoding::Utf16Le),
        ([vec![0xfe, 0xff], "x\u{00e9}".encode_utf16().flat_map(|u| u.to_be_bytes()).collect()].concat(), TextEncoding::Utf16Be),
    ];
    for (bytes, enc) in cases {
        let doc = TextDocument::open(Cursor::new(bytes.clone()), 1 << 20, ok0)?;
        assert_eq!(doc.encoding(), enc); assert_eq!(save(&doc), bytes, "{enc:?} roundtrip");
    }
    let mut doc = TextDocument::open(Cursor::new("\u{043f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}".as_bytes().to_vec()), 64, ok0)?;
    assert!(doc.replace(0, 1..2, "x").is_err(), "non-boundary edit accepted");
    let r1 = doc.replace(0, 0..2, "P")?; assert_eq!(doc.text(), "P\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}");
    assert_eq!(code(doc.replace(0, 0..1, "Q")), ErrorCode::SourceChanged);
    assert!(doc.replace(r1, 0..1, &"x".repeat(100)).is_err());
    assert!(doc.replace(r1, 0..1, "\0").is_err());
    assert!(TextDocument::open(Cursor::new(vec![0xc3, 0x28]), 64, ok0).is_err());
    assert!(TextDocument::open(Cursor::new(b"a\0b".to_vec()), 64, ok0).is_err());
    assert!(TextDocument::open(Cursor::new(vec![0xff, 0xfe, 0x00, 0xd8]), 64, ok0).is_err(), "lone surrogate");
    assert!(TextDocument::open(Cursor::new(vec![0xff, 0xfe, 0x41]), 64, ok0).is_err(), "odd UTF-16");
    assert_eq!(code(TextDocument::open(Cursor::new(vec![b'a'; 65]), 64, ok0)), ErrorCode::LimitExceeded);
    assert!(TextDocument::open(Cursor::new(Vec::new()), 0, ok0).is_err());
    // Save As through local storage never replaces.
    let h = H::new();
    let doc = TextDocument::open(Cursor::new(b"# md".to_vec()), 64, ok0)?;
    doc.save_as(h.files.as_ref(), &rp("note.md"), ok0)?;
    assert_eq!(std::fs::read(h.fpath("note.md"))?, b"# md");
    assert!(doc.save_as(h.files.as_ref(), &rp("note.md"), ok0).is_err());
    let reopened = TextDocument::open_location(&h.core, &h.loc(&h.files, "note.md"), 64, ok0)?;
    assert_eq!(reopened.text(), "# md");
    let _ = Read::bytes(Cursor::new(b""));
    Ok(())
}

/// E2.1: "needs a password" and "wrong password" are told apart from damage,
/// encrypted entries are marked, and nothing is written in either case.
#[test] fn archive_password_required_and_wrong_are_distinct() -> TestResult {
    let h = H::new();
    let plain = zip_open(our_zip(None))?;
    assert!(!any_encrypted(plain.entries()));
    let aes = our_zip(Some("correct horse"));
    let listed = zip_open(aes.clone())?;
    assert!(listed.entries().iter().all(|e| e.encrypted != e.directory), "files encrypted, folders not");
    let err = zip_extract(&h, aes.clone(), "nopw", None).unwrap_err();
    assert!(is_password_required(&err), "{err:?}");
    assert!(!h.fpath("nopw").exists(), "nothing created without a password");
    let err = zip_extract(&h, aes.clone(), "wrong", Some("battery staple")).unwrap_err();
    assert!(is_wrong_password(&err), "{err:?}");
    assert!(!h.fpath("wrong").exists(), "nothing created with a wrong password");
    zip_extract(&h, aes, "right", Some("correct horse"))?; assert_tree(&h.fpath("right"));
    if !interop() { return Ok(()); }
    let dir = h.temp.0.join("pw"); write_tree(&dir.join("src"));
    let names = ["docs", "data", "empty"];
    // Legacy ZipCrypto (Info-ZIP -P): marked encrypted, password asked for.
    let crypto = dir.join("zipcrypto.zip");
    run(Command::new("zip").current_dir(dir.join("src")).arg("-qr").arg("-Pk3y").arg(&crypto).args(names));
    let bytes = std::fs::read(&crypto)?;
    assert!(any_encrypted(zip_open(bytes.clone())?.entries()));
    assert!(is_password_required(&zip_extract(&h, bytes.clone(), "zc-nopw", None).unwrap_err()));
    zip_extract(&h, bytes, "zc", Some("k3y"))?; assert_tree(&h.fpath("zc"));
    // 7z with encrypted content (readable names) and with encrypted names.
    for (name, args) in [("data", &["-pk3y"][..]), ("names", &["-pk3y", "-mhe=on"][..])] {
        let out = dir.join(format!("{name}.7z"));
        run(Command::new("7z").current_dir(dir.join("src")).args(["a", "-bd", "-t7z"]).args(args).arg(&out).args(names));
        let bytes = std::fs::read(&out)?;
        let spool = |t: &str| h.spool.as_ref().metadata(&rp(t)).is_err();
        match sz_open(bytes.clone(), None) {
            Ok(mut s) => {
                assert_eq!(name, "data", "encrypted names cannot be listed without a password");
                assert!(any_encrypted(s.entries()));
                let err = s.prepare_extraction(h.spool.as_ref(), &rp("sz-nopw"), ok).unwrap_err();
                assert!(is_password_required(&err), "{name}: {err:?}");
                assert!(spool("sz-nopw"));
            }
            Err(err) => { assert_eq!(name, "names"); assert!(is_password_required(&err), "{err:?}"); }
        }
        let wrong = sz_open(bytes.clone(), Some("bad")).and_then(|mut s| s.prepare_extraction(h.spool.as_ref(), &rp(&format!("{name}-bad")), ok));
        let err = wrong.err().unwrap_or_else(|| panic!("{name}: wrong password accepted"));
        assert!(is_wrong_password(&err), "{name}: {err:?}");
        assert!(spool(&format!("{name}-bad")), "{name}: data left after a wrong password");
        sz_open(bytes, Some("k3y"))?.prepare_extraction(h.spool.as_ref(), &rp(&format!("{name}-ok")), ok)?;
    }
    Ok(())
}
