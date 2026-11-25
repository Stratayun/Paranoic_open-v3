//! Block 2 against files made by independent producers (see
//! tools/interop/block2/make_fixtures.py): python-docx, openpyxl, odfpy,
//! LibreOffice, reportlab, Pillow, ffmpeg. PDF goes through the real
//! out-of-process worker with PDFium.
//! Env: PARANOIC_BLOCK2_FIXTURES=<dir>; for PDF also PARANOIC_PDFIUM=<libpdfium.so>
//! and a worker built with `cargo build -p paranoic-documents --features pdf
//! --bin paranoic-pdf-worker` (or PARANOIC_PDF_WORKER=<path>). Unset = skipped.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::Cursor, path::PathBuf, sync::Arc, time::Duration};
use paranoic_core::*;
use paranoic_documents::*;
use paranoic_local::{Access, LocalStorage};

fn fixtures() -> Option<PathBuf> {
    match std::env::var("PARANOIC_BLOCK2_FIXTURES") {
        Ok(dir) => Some(PathBuf::from(dir)),
        Err(_) => { interop_skip("PARANOIC_BLOCK2_FIXTURES"); None }
    }
}
fn read(dir: &std::path::Path, name: &str) -> Vec<u8> { std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")) }
fn office(dir: &std::path::Path, name: &str) -> OfficeView {
    open_office(Cursor::new(read(dir, name)), OfficeLimits::default(), || Ok(())).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}
fn text(runs: &[Run]) -> String { runs.iter().map(|r| r.text.as_str()).collect() }

#[test] fn docx_from_python_docx_and_libreoffice() {
    let Some(dir) = fixtures() else { return };
    for name in ["python.docx", "libreoffice.docx"] {
        let OfficeView::Word(view) = office(&dir, name) else { panic!("{name}: not a document") };
        assert!(!view.truncated, "{name}");
        let paragraphs: Vec<(String, Option<u8>, bool)> = view.blocks.iter().filter_map(|b| match b {
            Block::Paragraph { heading, list, runs, .. } if !text(runs).is_empty() => Some((text(runs), *heading, *list)),
            _ => None,
        }).collect();
        assert_eq!(paragraphs, vec![
            ("Отчёт Paranoic".to_string(), Some(0), false), ("Раздел один".into(), Some(1), false),
            ("Обычный жирный и курсив и подчёркнутый".into(), None, false), ("пункт списка".into(), None, true),
        ], "{name}");
        let formatted = view.blocks.iter().find_map(|b| match b {
            Block::Paragraph { runs, .. } if text(runs).starts_with("Обычный") => Some(runs.clone()), _ => None }).unwrap();
        let style = |word: &str| formatted.iter().find(|r| r.text == word).map(|r| (r.bold, r.italic, r.underline));
        assert_eq!(style("жирный"), Some((true, false, false)), "{name}");
        assert_eq!(style("курсив"), Some((false, true, false)), "{name}");
        assert_eq!(style("подчёркнутый"), Some((false, false, true)), "{name}");
        let table = view.blocks.iter().find_map(|b| match b { Block::Table { rows } => Some(rows.clone()), _ => None }).unwrap();
        assert_eq!(table, vec![vec!["A1".to_string(), "B1".into()], vec!["A2".into(), "Б2".into()]], "{name}");
    }
}

#[test] fn spreadsheets_from_openpyxl_odfpy_and_libreoffice() {
    let Some(dir) = fixtures() else { return };
    for name in ["openpyxl.xlsx", "libreoffice.xlsx", "libreoffice.ods"] {
        let OfficeView::Workbook(view) = office(&dir, name) else { panic!("{name}: not a workbook") };
        assert!(!view.truncated, "{name}");
        let names: Vec<&str> = view.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Данные", "Второй"], "{name}");
        let rows = &view.sheets[0].rows;
        assert_eq!(rows[0], ["Имя", "Число", "Дата"], "{name}");
        assert_eq!(rows[1], ["альфа", "42", "2024-02-29"], "{name}");
        assert_eq!(rows[2], ["бета", "3.5", "1999-12-31"], "{name}");
        // openpyxl stores the formula without a cached value; LibreOffice recalculates.
        let formula = rows[3].get(1).map(String::as_str).unwrap_or("");
        assert_eq!(formula, if name == "openpyxl.xlsx" { "" } else { "84" }, "{name}");
        assert_eq!(rows[9].get(4).map(String::as_str), Some("далеко"), "{name}: sparse cell E10");
        assert_eq!(view.sheets[1].rows, vec![vec!["второй лист".to_string()]], "{name}");
    }
    // 1,000,000 repeated rows x 16,384 repeated cells (empty) cost nothing.
    let OfficeView::Workbook(view) = office(&dir, "odfpy.ods") else { panic!("odfpy.ods") };
    assert_eq!(view.sheets[0].rows, vec![vec!["x".to_string(), "1".into()], vec!["y".into(), "2".into()]]);
}

#[test] fn images_audio_video_are_classified_and_bounded() {
    let Some(dir) = fixtures() else { return };
    for (name, mime) in [("image.png", "image/png"), ("image.jpg", "image/jpeg"), ("image.gif", "image/gif"),
        ("image.webp", "image/webp"), ("image.bmp", "image/bmp")] {
        let bytes = read(&dir, name);
        let media = sniff(&bytes[..64.min(bytes.len())]).unwrap_or_else(|| panic!("{name}: not recognised"));
        assert_eq!((media.kind, media.mime, media.decodable), (MediaKind::Image, mime, true), "{name}");
        let image = decode_image(&bytes, ImageLimits::default()).unwrap();
        assert_eq!((image.width, image.height), (300, 120), "{name}");
        let thumb = thumbnail_image(&bytes, 64, ImageLimits::default()).unwrap();
        let small = decode_image(&thumb, ImageLimits::default()).unwrap();
        assert_eq!((small.width, small.height), (64, 26), "{name}: aspect kept");
    }
    // EXIF orientation 6 is applied (stored 300x120, shown 120x300).
    let rotated = decode_image(&read(&dir, "rotated.jpg"), ImageLimits::default()).unwrap();
    assert_eq!((rotated.width, rotated.height), (120, 300));
    // 60000x60000 declared in a small file: refused before allocation.
    assert!(matches!(decode_image(&read(&dir, "bomb.png"), ImageLimits::default()), Err(CoreError { code: ErrorCode::LimitExceeded, .. })));
    for (name, kind, mime) in [("audio.wav", MediaKind::Audio, "audio/wav"), ("audio.flac", MediaKind::Audio, "audio/flac"),
        ("audio.ogg", MediaKind::Audio, "audio/ogg"), ("audio.mp3", MediaKind::Audio, "audio/mpeg"),
        ("audio.m4a", MediaKind::Audio, "audio/mp4"), ("video.webm", MediaKind::Video, "video/webm"),
        ("video.mp4", MediaKind::Video, "video/mp4"), ("video.mkv", MediaKind::Video, "video/x-matroska")] {
        let bytes = read(&dir, name);
        let media = sniff(&bytes[..64.min(bytes.len())]).unwrap_or_else(|| panic!("{name}: not recognised"));
        assert_eq!((media.kind, media.mime, media.decodable), (kind, mime, false), "{name}");
        assert!(matches!(decode_image(&bytes, ImageLimits::default()), Err(CoreError { code: ErrorCode::Unsupported, .. })), "{name}");
    }
    assert!(sniff(&read(&dir, "three-pages.pdf")[..64]).is_none(), "PDF is not media");
}

// ---------- PDF through the worker ----------
struct Pdf { core: Core, storage: Arc<LocalStorage>, worker: PathBuf, pdfium: PathBuf, _state: tempdir::Dir }
mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
}
fn pdf_env() -> Option<Pdf> {
    let dir = fixtures()?;
    let Ok(pdfium) = std::env::var("PARANOIC_PDFIUM") else { interop_skip("PARANOIC_PDFIUM"); return None };
    let worker = std::env::var("PARANOIC_PDF_WORKER").map(PathBuf::from).unwrap_or_else(|_|
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/paranoic-pdf-worker"));
    let worker = worker.canonicalize().expect("PDF worker not built");
    let work = std::env::temp_dir().join(format!("paranoic-pdf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(work.join("files")).unwrap();
    for name in ["three-pages.pdf", "image.png"] { std::fs::copy(dir.join(name), work.join("files").join(name)).unwrap(); }
    let mut bad = std::fs::read(dir.join("three-pages.pdf")).unwrap();
    let half = bad.len() / 2; bad.truncate(half);
    std::fs::write(work.join("files/truncated.pdf"), &bad).unwrap();
    std::fs::write(work.join("files/garbage.pdf"), [b"%PDF-1.7\n".as_slice(), &[0xa5; 4000]].concat()).unwrap();
    let storage = Arc::new(LocalStorage::open(StorageId(uuid::Uuid::new_v4()), work.join("files"), Access::ReadWrite).unwrap());
    let mut registry = StorageRegistry::new(); registry.register(storage.clone()).unwrap();
    let core = Core::open(work.join("state"), CoreConfig::default(), registry).unwrap();
    Some(Pdf { core, storage, worker, pdfium: PathBuf::from(pdfium), _state: tempdir::Dir(work) })
}
impl Pdf {
    fn open(&self, name: &str) -> paranoic_core::Result<pdf_client::PdfSession> {
        let location = Location { storage: self.storage.id(), path: ResourcePath::parse(name).unwrap() };
        pdf_client::PdfSession::open(&self.core, &location, &self.worker, &self.pdfium)
    }
}
/// Worker processes that are children of this test process.
fn workers() -> Vec<u32> {
    let me = std::process::id();
    std::fs::read_dir("/proc").unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok()).filter(|pid| {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        comm.starts_with("paranoic-pdf") && status.lines().any(|l| l == format!("PPid:\t{me}"))
            && !status.lines().any(|l| l.starts_with("State:\tZ"))
    }).collect()
}

#[test] fn pdf_renders_searches_and_runs_confined() {
    let Some(env) = pdf_env() else { return };
    let mut session = env.open("three-pages.pdf").unwrap();
    assert_eq!(session.pages(), 3);
    let page = session.render(0, 400).unwrap();
    let image = decode_image(&page, ImageLimits::default()).unwrap();
    assert_eq!(image.width, 400, "rendered at the requested width");
    assert!(image.rgba.chunks(4).any(|p| p[0] < 128), "page has visible text");
    assert_eq!(session.search("alpha", 0).unwrap(), (vec![0, 2], None));
    assert_eq!(session.search("beta", 0).unwrap(), (vec![1], None));
    assert_eq!(session.search("gamma", 0).unwrap(), (vec![], None));
    assert!(session.render(3, 400).is_err(), "page past the end");
    assert!(session.render(0, 100_000).is_err(), "oversized render refused");
    // The live worker runs under seccomp (filter mode), no_new_privs and rlimits.
    let pids = workers();
    assert_eq!(pids.len(), 1, "exactly one worker: {pids:?}");
    let status = std::fs::read_to_string(format!("/proc/{}/status", pids[0])).unwrap();
    assert!(status.lines().any(|l| l == "Seccomp:\t2"), "worker not under a seccomp filter:\n{status}");
    assert!(status.lines().any(|l| l == "NoNewPrivs:\t1"), "no_new_privs not set");
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", pids[0])).unwrap();
    let line = |name: &str| limits.lines().find(|l| l.starts_with(name)).unwrap_or("").split_whitespace().rev().nth(1).unwrap_or("").to_string();
    assert_eq!(line("Max file size"), "0", "file growth not blocked:\n{limits}");
    assert_eq!(line("Max cpu time"), "120");
    assert!(session.is_alive());
    drop(session);
    std::thread::sleep(Duration::from_millis(200));
    assert!(workers().is_empty(), "worker survived its session");
}

/// E4.7: a PDF decrypted in memory (inside a vault) goes to the same worker.
#[test] fn pdf_opens_from_memory_bytes() {
    let Some(env) = pdf_env() else { return };
    let bytes = std::fs::read(env._state.0.join("files/three-pages.pdf")).unwrap();
    let source = paranoic_core::Metadata { kind: paranoic_core::EntryKind::File, bytes: bytes.len() as u64, modified_unix_ms: Some(1) };
    let mut session = pdf_client::PdfSession::open_bytes_with_host(zeroize::Zeroizing::new(bytes), source.clone(), &env.worker, &env.pdfium, None).unwrap();
    assert_eq!(session.pages(), 3);
    assert_eq!(session.search("beta", 0).unwrap(), (vec![1], None));
    assert!(pdf_client::PdfSession::open_bytes_with_host(zeroize::Zeroizing::new(b"not a pdf".to_vec()), source, &env.worker, &env.pdfium, None).is_err());
}

#[test] fn pdf_rejects_broken_and_non_pdf_input_without_leaking_workers() {
    let Some(env) = pdf_env() else { return };
    for name in ["truncated.pdf", "garbage.pdf", "image.png"] {
        let result = env.open(name);
        assert!(result.is_err(), "{name} opened as PDF");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(workers().is_empty(), "failed opens left workers: {:?}", workers());
    // A missing PDFium is a capability error, not a crash.
    let location = Location { storage: env.storage.id(), path: ResourcePath::parse("three-pages.pdf").unwrap() };
    let missing = pdf_client::PdfSession::open(&env.core, &location, &env.worker, std::path::Path::new("/nonexistent/libpdfium.so"));
    assert!(missing.is_err());
    std::thread::sleep(Duration::from_millis(200));
    assert!(workers().is_empty());
}

/// Audit 18: with PARANOIC_REQUIRE_INTEROP set, a missing fixture or tool fails
/// the run instead of passing as a silent skip.
#[allow(dead_code)]
fn interop_skip(what: &str) {
    assert!(std::env::var_os("PARANOIC_REQUIRE_INTEROP").is_none(), "{what} required by PARANOIC_REQUIRE_INTEROP");
    eprintln!("{what}: skipped");
}
