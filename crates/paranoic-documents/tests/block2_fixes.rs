//! Block 2 regressions without external files: 7z header pre-check (encoded
//! header bounds, SubStreamsInfo), version-checked text save, hostile Office
//! packages and the cell budget, DOCX list detection, encrypted thumbnail
//! cache, MediaStream random access and change detection.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::{Cursor, Write}, path::PathBuf, sync::Arc, time::{Duration, Instant}};
use paranoic_core::*;
use paranoic_documents::*;
use paranoic_local::{Access, LocalStorage};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-b2f-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap(); }
        Self(p)
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
    fn file(&self, rel: &str) -> PathBuf { self.temp.0.join("files").join(rel) }
    fn loc(&self, storage: &LocalStorage, p: &str) -> Location { Location { storage: storage.id(), path: ResourcePath::parse(p).unwrap() } }
    fn wait(&self, id: JobId) -> JobRecord {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let job = self.core.job(id).unwrap();
            if !matches!(job.state, JobState::Queued | JobState::Running) { return job; }
            assert!(Instant::now() < until, "job timed out"); std::thread::sleep(Duration::from_millis(5));
        }
    }
}
fn code<T>(r: paranoic_core::Result<T>) -> ErrorCode { match r { Err(e) => e.code, Ok(_) => panic!("expected an error") } }

// ---------- 7z header pre-check ----------
/// A 7z file whose next header is `next` (packed stream: 5 garbage bytes).
fn seven_zip(next: &[u8]) -> Vec<u8> {
    let packed = [0u8; 5];
    let mut tail = Vec::new();
    tail.extend_from_slice(&(packed.len() as u64).to_le_bytes());
    tail.extend_from_slice(&(next.len() as u64).to_le_bytes());
    tail.extend_from_slice(&crc32fast::hash(next).to_le_bytes());
    let mut out = b"7z\xbc\xaf\x27\x1c\x00\x04".to_vec();
    out.extend_from_slice(&crc32fast::hash(&tail).to_le_bytes());
    out.extend_from_slice(&tail); out.extend_from_slice(&packed); out.extend_from_slice(next);
    out
}
/// kEncodedHeader with one LZMA folder: `props` (5 bytes), unpacked `size`, then `extra` before the end marker.
fn encoded_header(props: [u8; 5], size: u64, extra: &[u8]) -> Vec<u8> {
    let mut h = vec![0x17, 0x06, 0x00, 0x01, 0x09, 0x05, 0x00, 0x07, 0x0b, 0x01, 0x00, 0x01, 0x23, 0x03, 0x01, 0x01, 0x05];
    h.extend_from_slice(&props);
    h.push(0x0c); h.push(0xff); h.extend_from_slice(&size.to_le_bytes());
    h.extend_from_slice(&[0x0a, 0x01, 1, 2, 3, 4, 0x00]); // folder CRC defined, end of UnpackInfo
    h.extend_from_slice(extra);
    h.push(0x00);
    h
}
fn open_7z(bytes: Vec<u8>) -> paranoic_core::Result<SevenZipService<Cursor<Vec<u8>>>> {
    SevenZipService::open(Cursor::new(bytes), CodecLimits::default(), None, || Ok(()))
}

#[test] fn sevenz_encoded_header_is_bounded_before_decoding() {
    let small_dict = [0x5d, 0x00, 0x00, 0x01, 0x00];
    // Declared unpacked header of 1 TiB: refused without inflating.
    assert_eq!(code(open_7z(seven_zip(&encoded_header(small_dict, 1 << 40, &[])))), ErrorCode::LimitExceeded);
    // 2 GiB LZMA dictionary for the header stream.
    assert_eq!(code(open_7z(seven_zip(&encoded_header([0x5d, 0, 0, 0, 0x80], 100, &[])))), ErrorCode::LimitExceeded);
    // SubStreamsInfo (as written by sevenz-rust2) is understood...
    let substreams = [0x08, 0x0d, 0x01, 0x09, 0x0a, 0x01, 0x00];
    let parsed = open_7z(seven_zip(&encoded_header(small_dict, 100, &substreams)));
    assert!(!matches!(parsed, Err(CoreError { code: ErrorCode::LimitExceeded, .. })), "valid layout must pass the pre-check");
    // ...but an absurd number of streams in the header folder is refused.
    let many = [0x08, 0x0d, 0x7f, 0x00];
    let refused = open_7z(seven_zip(&encoded_header(small_dict, 100, &many)));
    assert!(matches!(&refused, Err(e) if e.message == "7z header layout rejected"), "refused by the pre-check: {:?}", refused.err());
    // SubStreamsInfo without UnpackInfo is malformed.
    assert_eq!(code(open_7z(seven_zip(&[0x17, 0x08, 0x00, 0x00]))), ErrorCode::InvalidInput);
}

// ---------- text save ----------
#[test] fn private_7z_preparation_batches_files_and_cleans_prior_batches() -> TestResult {
    let h = H::new();
    let sources = (0..129).map(|i| ZipSource { path: ResourcePath::parse(format!("f{i:03}" )).unwrap(),
        bytes: 1, reader: Some(Box::new(Cursor::new(vec![i as u8]))) }).collect();
    let mut encoded = Cursor::new(Vec::new());
    SevenZipService::create(&mut encoded, sources, CodecLimits::default(), |_| Ok(()))?;
    let bytes = encoded.into_inner();
    let limits = CodecLimits { pending_files: 2, ..CodecLimits::default() };
    let mut archive = SevenZipService::open(Cursor::new(bytes.clone()), limits, None, || Ok(()))?;
    let ready = archive.prepare_extraction(h.spool.as_ref(), &ResourcePath::parse("many")?, |_| Ok(()))?;
    assert_eq!(ready.files().len(), 129);
    assert_eq!(std::fs::read(h.temp.0.join("spool/many/f128"))?, [128]);
    let mut interrupted = SevenZipService::open(Cursor::new(bytes), limits, None, || Ok(()))?;
    assert!(interrupted.prepare_extraction(h.spool.as_ref(), &ResourcePath::parse("cancelled")?, |n|
        if n >= 3 { Err(CoreError::new(ErrorCode::Cancelled, "test cancellation")) } else { Ok(()) }).is_err());
    assert!(!h.temp.0.join("spool/cancelled").exists(), "earlier committed private batches leaked");
    Ok(())
}

struct StageSink(Option<Box<dyn StagedWrite>>);
impl Write for StageSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.as_mut().ok_or_else(|| std::io::Error::other("closed"))?.write(data) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl paranoic_crypto::StreamSink for StageSink {
    fn commit(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        self.0.take().ok_or(paranoic_crypto::CryptoError::Io)?.publish().map_err(|_| paranoic_crypto::CryptoError::Io)
    }
    fn abort(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        if let Some(stage) = self.0.take() { let _ = stage.abort(); } Ok(())
    }
}

#[test] fn text_save_never_overwrites_an_external_edit() -> TestResult {
    let h = H::new();
    std::fs::write(h.file("note.md"), "# заметка\n")?;
    let mut doc = TextDocument::open_location(&h.core, &h.loc(&h.files, "note.md"), 1 << 20, || Ok(()))?;
    let opened = doc.opened_metadata().cloned().expect("opened version recorded");
    doc.replace(doc.revision(), 0..1, "##")?;
    // Someone else edits the file meanwhile (different size).
    std::fs::write(h.file("note.md"), "# заметка\nчужая правка\n")?;
    let spool = ResourcePath::parse("save-1")?;
    let artifact = doc.prepare_save(Box::new(StageSink(Some(h.spool.stage(&spool, JobId(uuid::Uuid::new_v4()))?))), || Ok(()))?;
    let job = JobId(uuid::Uuid::new_v4());
    artifact.submit_replace(&h.core, h.loc(&h.spool, "save-1"), h.loc(&h.files, "note.md"), opened.clone(), job)?;
    let done = h.wait(job);
    assert_ne!(done.state, JobState::Succeeded, "{done:?}");
    assert_eq!(std::fs::read_to_string(h.file("note.md"))?, "# заметка\nчужая правка\n", "external edit kept");
    // Against the current version the save goes through.
    let current = h.files.metadata(&ResourcePath::parse("note.md")?)?;
    let job = JobId(uuid::Uuid::new_v4());
    artifact.submit_replace(&h.core, h.loc(&h.spool, "save-1"), h.loc(&h.files, "note.md"), current, job)?;
    assert_eq!(h.wait(job).state, JobState::Succeeded);
    assert_eq!(std::fs::read_to_string(h.file("note.md"))?, "## заметка\n");
    Ok(())
}

// ---------- Office ----------
#[test] fn queued_text_save_checks_contents_even_with_unchanged_metadata() -> TestResult {
    let h = H::new();
    std::fs::write(h.file("same.txt"), b"original")?;
    let mut doc = TextDocument::open_location(&h.core, &h.loc(&h.files, "same.txt"), 1024, || Ok(()))?;
    let old = doc.opened_metadata().cloned().unwrap();
    let hash = *doc.original_blake3();
    let mtime = std::fs::metadata(h.file("same.txt"))?.modified()?;
    doc.replace(doc.revision(), 0..doc.text().len(), "my edit")?;
    let spool = ResourcePath::parse("checked-save")?;
    let artifact = doc.prepare_save(Box::new(StageSink(Some(h.spool.stage(&spool, JobId(uuid::Uuid::new_v4()))?))), || Ok(()))?;
    // Already changed before enqueue, but the metadata expectation still matches.
    std::fs::write(h.file("same.txt"), b"external")?;
    std::fs::File::options().write(true).open(h.file("same.txt"))?.set_modified(mtime)?;
    let id = JobId(uuid::Uuid::new_v4());
    artifact.submit_replace_content(&h.core, h.loc(&h.spool, "checked-save"), h.loc(&h.files, "same.txt"), old, hash, id)?;
    assert_ne!(h.wait(id).state, JobState::Succeeded);
    assert_eq!(std::fs::read(h.file("same.txt"))?, b"external");
    Ok(())
}

fn package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}
const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const DOC_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
const DOC_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="s1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
fn docx(body: &str, styles: &str) -> Vec<u8> {
    let document = format!(r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#);
    let styles = format!(r#"<?xml version="1.0"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{styles}</w:styles>"#);
    package(&[("[Content_Types].xml", DOC_TYPES), ("_rels/.rels", RELS), ("word/_rels/document.xml.rels", DOC_RELS),
        ("word/document.xml", &document), ("word/styles.xml", &styles)])
}
fn word(bytes: Vec<u8>, limits: OfficeLimits) -> paranoic_core::Result<WordView> {
    match open_office(Cursor::new(bytes), limits, || Ok(()))? { OfficeView::Word(w) => Ok(w), _ => panic!("not a document") }
}
fn para(style: &str, num: &str, text: &str) -> String {
    let ppr = if style.is_empty() && num.is_empty() { String::new() } else {
        let style = if style.is_empty() { String::new() } else { format!(r#"<w:pStyle w:val="{style}"/>"#) };
        let num = if num.is_empty() { String::new() } else { format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num}"/></w:numPr>"#) };
        format!("<w:pPr>{style}{num}</w:pPr>")
    };
    format!("<w:p>{ppr}<w:r><w:t>{text}</w:t></w:r></w:p>")
}

#[test] fn docx_lists_from_paragraph_and_style_chain() -> TestResult {
    let styles = r#"<w:style w:type="paragraph" w:styleId="Bullets"><w:name w:val="Bullets"/><w:pPr><w:numPr><w:numId w:val="3"/></w:numPr></w:pPr></w:style>
        <w:style w:type="paragraph" w:styleId="MyBullets"><w:name w:val="Mine"/><w:basedOn w:val="Bullets"/></w:style>
        <w:style w:type="paragraph" w:styleId="NoList"><w:name w:val="NoList"/><w:basedOn w:val="Bullets"/><w:pPr><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr></w:style>
        <w:style w:type="paragraph" w:styleId="LoopA"><w:name w:val="A"/><w:basedOn w:val="LoopB"/></w:style>
        <w:style w:type="paragraph" w:styleId="LoopB"><w:name w:val="B"/><w:basedOn w:val="LoopA"/></w:style>
        <w:style w:type="paragraph" w:styleId="H2"><w:name w:val="heading 2"/></w:style>"#;
    let body = [para("", "5", "direct"), para("Bullets", "", "style"), para("MyBullets", "", "inherited"),
        para("NoList", "", "cleared"), para("Bullets", "0", "paragraph clears"), para("LoopA", "", "cycle"),
        para("H2", "", "heading"), para("", "", "plain")].concat();
    let view = word(docx(&body, styles), OfficeLimits::default())?;
    let got: Vec<(String, bool, Option<u8>)> = view.blocks.iter().map(|b| match b {
        Block::Paragraph { list, heading, runs, .. } => (runs.iter().map(|r| r.text.as_str()).collect(), *list, *heading),
        Block::Table { .. } => panic!("unexpected table"),
    }).collect();
    let expected = [("direct", true, None), ("style", true, None), ("inherited", true, None), ("cleared", false, None),
        ("paragraph clears", false, None), ("cycle", false, None), ("heading", false, Some(2)), ("plain", false, None)];
    assert_eq!(got, expected.iter().map(|(t, l, h)| (t.to_string(), *l, *h)).collect::<Vec<_>>());
    Ok(())
}

#[test] fn office_rejects_dtd_and_deep_nesting() {
    // A DOCTYPE alone (no entity use) is refused: no DTD processing at all.
    let dtd = r#"<!DOCTYPE w:document [<!ENTITY x "boom">]>"#;
    let body = "<w:p><w:r><w:t>plain</w:t></w:r></w:p>";
    // Inject the DOCTYPE into document.xml.
    let with_dtd = {
        let document = format!(r#"<?xml version="1.0"?>{dtd}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#);
        package(&[("[Content_Types].xml", DOC_TYPES), ("_rels/.rels", RELS), ("word/document.xml", &document)])
    };
    assert!(matches!(word(with_dtd, OfficeLimits::default()), Err(CoreError { code: ErrorCode::Unsupported, .. })), "DTD must be rejected");
    let deep = format!("<w:p>{}<w:r><w:t>x</w:t></w:r>{}</w:p>", "<w:sdt>".repeat(300), "</w:sdt>".repeat(300));
    assert!(word(docx(&deep, ""), OfficeLimits::default()).is_err(), "nesting beyond xml_depth must fail");
}

const XLSX_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
const XLSX_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
const WB: &str = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
const WB_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
fn xlsx(cells: &str) -> Vec<u8> {
    let sheet = format!(r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{cells}</sheetData></worksheet>"#);
    package(&[("[Content_Types].xml", XLSX_TYPES), ("_rels/.rels", XLSX_RELS), ("xl/workbook.xml", WB),
        ("xl/_rels/workbook.xml.rels", WB_RELS), ("xl/worksheets/sheet1.xml", &sheet)])
}
fn workbook(bytes: Vec<u8>, limits: OfficeLimits) -> paranoic_core::Result<WorkbookView> {
    match open_office(Cursor::new(bytes), limits, || Ok(()))? { OfficeView::Workbook(w) => Ok(w), _ => panic!("not a workbook") }
}

fn xlsx_shared(strings: &str) -> Vec<u8> {
    let rels = WB_RELS.replace("</Relationships>", r#"<Relationship Id="ss" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#);
    let shared = format!("<sst>{strings}</sst>");
    let sheet = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#;
    package(&[("[Content_Types].xml", XLSX_TYPES), ("_rels/.rels", XLSX_RELS), ("xl/workbook.xml", WB),
        ("xl/_rels/workbook.xml.rels", &rels), ("xl/sharedStrings.xml", &shared), ("xl/worksheets/sheet1.xml", sheet)])
}

#[test] fn spreadsheet_auxiliary_tables_and_fragments_are_bounded() -> TestResult {
    let many = xlsx_shared(&"<si/>".repeat(64));
    assert_eq!(code(workbook(many, OfficeLimits { auxiliary_items: 16, ..OfficeLimits::default() })), ErrorCode::LimitExceeded);
    let huge = xlsx_shared(&format!("<si><t>{}</t></si>", "я".repeat(5000)));
    let view = workbook(huge, OfficeLimits { cell_chars: 8, ..OfficeLimits::default() })?;
    assert_eq!(view.sheets[0].rows[0][0], "яяяя");
    assert!(view.truncated);
    let fragments = xlsx_shared(&format!("<si><t>{}</t></si>", "x&amp;".repeat(128)));
    assert_eq!(code(workbook(fragments, OfficeLimits { xml_events: 32, ..OfficeLimits::default() })), ErrorCode::LimitExceeded);
    Ok(())
}

#[test] fn spreadsheet_row_overflow_is_rejected_without_panicking() {
    assert!(workbook(xlsx(r#"<row r="18446744073709551615"/><row/>"#), OfficeLimits::default()).is_err());
}

#[test] fn spreadsheet_cell_budget_bounds_sparse_and_dense_sheets() -> TestResult {
    // A sparse address far away would materialise ~16k*1k slots; the budget stops it.
    let sparse = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row><row r="1000"><c r="IV1000" t="inlineStr"><is><t>far</t></is></c></row>"#;
    let limits = OfficeLimits { cells: 1000, ..OfficeLimits::default() };
    let view = workbook(xlsx(sparse), limits)?;
    let slots: usize = view.sheets.iter().map(|s| s.rows.len() + s.rows.iter().map(Vec::len).sum::<usize>()).sum();
    assert!(view.truncated || view.sheets.iter().any(|s| s.truncated), "budget exhaustion must be reported");
    assert!(slots <= 1000, "materialised {slots} slots over a budget of 1000");
    // Dense: 50 x 50 inline cells with a budget of 500.
    let dense: String = (1..=50).map(|r| format!("<row r=\"{r}\">{}</row>",
        (0..50).map(|c| format!("<c t=\"inlineStr\"><is><t>{r}-{c}</t></is></c>")).collect::<String>())).collect();
    let view = workbook(xlsx(&dense), OfficeLimits { cells: 500, ..OfficeLimits::default() })?;
    let slots: usize = view.sheets.iter().map(|s| s.rows.len() + s.rows.iter().map(Vec::len).sum::<usize>()).sum();
    assert!(slots <= 500 && view.sheets.iter().any(|s| s.truncated) || view.truncated);
    // Within budget: complete and exact.
    let view = workbook(xlsx(&dense), OfficeLimits::default())?;
    assert_eq!(view.sheets[0].rows.len(), 50);
    assert_eq!(view.sheets[0].rows[49][49], "50-49");
    assert!(!view.truncated && !view.sheets[0].truncated);
    Ok(())
}

// ---------- thumbnails and media ----------
/// PNG padded after IEND to `length` bytes (decoders ignore trailing data), so
/// two different pictures can share one file size.
fn png(width: u32, height: u32, rgb: [u8; 3], length: usize) -> Vec<u8> {
    let rgba: Vec<u8> = (0..width * height).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
    let mut bytes = thumbnail_rgba(width, height, &rgba, width.max(height)).unwrap().to_vec();
    assert!(bytes.len() <= length); bytes.resize(length, 0); bytes
}

#[test] fn thumbnail_cache_is_encrypted_keyed_by_version_and_wiped_on_lock() -> TestResult {
    let h = H::new();
    let cache_dir = h.temp.0.join("cache"); std::fs::create_dir(&cache_dir)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o700))?; }
    // Leftover of a crashed session (its key is gone) is removed on start.
    std::fs::write(cache_dir.join("0123456789abcdef0123456789abcdef"), b"stale")?;
    let cache = ThumbnailCache::ephemeral(paranoic_crypto::PrivateDirectory::open(&cache_dir)?)?;
    assert!(!cache_dir.join("0123456789abcdef0123456789abcdef").exists());
    std::fs::write(h.file("photo.png"), png(64, 32, [10, 200, 30], 4096))?;
    let loc = h.loc(&h.files, "photo.png");
    let first = cached_thumbnail(&h.core, &cache, &loc, 16, ImageLimits::default(), || Ok(()))?;
    assert!(first.starts_with(b"\x89PNG"));
    // On disk: one object, no PNG signature, no plaintext.
    let objects: Vec<_> = std::fs::read_dir(&cache_dir)?.filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().len() == 32).collect();
    assert_eq!(objects.len(), 1);
    let stored = std::fs::read(objects[0].path())?;
    assert!(!stored.windows(4).any(|w| w == b"\x89PNG") && !stored.windows(4).any(|w| w == b"IHDR"), "thumbnail stored in plaintext");
    // Same size, same name, new content and mtime: a new thumbnail, not the stale one.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(h.file("photo.png"), png(64, 32, [250, 0, 0], 4096))?;
    assert_eq!(std::fs::metadata(h.file("photo.png"))?.len(), 4096);
    let second = cached_thumbnail(&h.core, &cache, &loc, 16, ImageLimits::default(), || Ok(()))?;
    assert_ne!(first.as_slice(), second.as_slice(), "stale thumbnail served after replacement");
    // Lock removes every object and refuses further use.
    cache.lock()?;
    assert!(cache.is_locked());
    assert_eq!(std::fs::read_dir(&cache_dir)?.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().len() == 32).count(), 0);
    assert!(cached_thumbnail(&h.core, &cache, &loc, 16, ImageLimits::default(), || Ok(())).is_err());
    Ok(())
}

#[test] fn media_stream_reads_randomly_and_detects_replacement() -> TestResult {
    let h = H::new();
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec(); wav.extend_from_slice(&data);
    std::fs::write(h.file("a.wav"), &wav)?;
    let mut stream = MediaStream::open(&h.core, h.loc(&h.files, "a.wav"))?;
    assert_eq!(stream.media_type().map(|m| m.mime), Some("audio/wav"));
    assert_eq!(stream.len(), wav.len() as u64);
    for offset in [250_000u64, 10, 199_999, 0, wav.len() as u64 - 3] {
        let mut buf = [0u8; 1000];
        let n = stream.read_at(offset, &mut buf)?;
        assert_eq!(&buf[..n], &wav[offset as usize..offset as usize + n], "offset {offset}");
    }
    let mut buf = [0u8; 10];
    assert_eq!(stream.read_at(wav.len() as u64, &mut buf)?, 0, "end of stream");
    // Truncated underneath the player: reported as a change, not as short data.
    std::fs::write(h.file("a.wav"), &wav[..1000])?;
    let mut big = [0u8; 4096];
    let result = stream.read_at(200_000, &mut big);
    assert!(matches!(result, Err(CoreError { code: ErrorCode::SourceChanged, .. })), "{result:?}");
    Ok(())
}
