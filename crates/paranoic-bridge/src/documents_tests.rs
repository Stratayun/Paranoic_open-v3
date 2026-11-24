//! Plan 2.2/2.3: text open/save (version-checked) and Office view through NativeRuntime.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf};

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

fn package(parts: &[(&str, &str)]) -> Vec<u8> {
    let sources = parts.iter().map(|(name, body)| paranoic_documents::ZipSource { path: paranoic_core::ResourcePath::parse(*name).unwrap(),
        bytes: body.len() as u64, reader: Some(Box::new(std::io::Cursor::new(body.as_bytes().to_vec()))) }).collect();
    let mut out = std::io::Cursor::new(Vec::new());
    paranoic_documents::ZipService::create(&mut out, sources, Default::default(), None, |_| Ok(())).unwrap();
    out.into_inner()
}
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub(crate) fn docx() -> Vec<u8> {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let body = r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Жирный</w:t></w:r><w:r><w:t> обычный</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let document = format!(r#"<?xml version="1.0"?><w:document xmlns:w="{W}"><w:body>{body}</w:body></w:document>"#);
    package(&[("[Content_Types].xml", types), ("_rels/.rels", rels), ("word/document.xml", &document)])
}
fn xlsx() -> Vec<u8> {
    let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let wb = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Лист" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let wb_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
    let sheet = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>имя</t></is></c><c r="B1"><v>42</v></c></row></sheetData></worksheet>"#;
    package(&[("[Content_Types].xml", types), ("_rels/.rels", rels), ("xl/workbook.xml", wb), ("xl/_rels/workbook.xml.rels", wb_rels), ("xl/worksheets/sheet1.xml", sheet)])
}

#[test] fn text_and_office_documents() {
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-docs-{}", Uuid::new_v4())));
    for d in ["state", "workspace"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let ws = fx.0.join("workspace");
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };

    // UTF-16 LE text with BOM: encoding kept on save.
    let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("# Заметка\nстрока".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
    fs::write(ws.join("note.md"), &utf16).unwrap();
    let doc = rt.text_open(at("note.md")).unwrap();
    assert_eq!((doc.text.as_str(), doc.encoding), ("# Заметка\nстрока", 3));
    let v2 = rt.text_save(at("note.md"), doc.version.clone(), "# Заметка\nновая строка".into()).unwrap();
    let saved = fs::read(ws.join("note.md")).unwrap();
    assert_eq!(&saved[..2], &[0xFF, 0xFE], "encoding lost");
    assert_eq!(rt.text_open(at("note.md")).unwrap().text, "# Заметка\nновая строка");
    assert_eq!(v2.blake3, blake3::hash(&saved).to_hex().as_str());
    assert!(fs::read_dir(&ws).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")),
        "successful save retained its spool");
    // A stale version (file changed since opening) is never overwritten.
    assert!(matches!(rt.text_save(at("note.md"), doc.version, "stale edit".into()), Err(BridgeError::Changed)));
    fs::write(ws.join("note.md"), "edited elsewhere, longer").unwrap();
    assert!(matches!(rt.text_save(at("note.md"), v2, "my edit".into()), Err(BridgeError::Changed)));
    assert_eq!(fs::read_to_string(ws.join("note.md")).unwrap(), "edited elsewhere, longer");
    // Same length and restored mtime: metadata is not an editor version token.
    fs::write(ws.join("same.txt"), b"original").unwrap();
    let original = rt.text_open(at("same.txt")).unwrap();
    let mtime = fs::metadata(ws.join("same.txt")).unwrap().modified().unwrap();
    fs::write(ws.join("same.txt"), b"external").unwrap();
    fs::File::options().write(true).open(ws.join("same.txt")).unwrap().set_modified(mtime).unwrap();
    assert!(matches!(rt.text_save(at("same.txt"), original.version, "my edit".into()), Err(BridgeError::Changed)));
    assert_eq!(fs::read(ws.join("same.txt")).unwrap(), b"external");
    // Save as: new UTF-8 file, never replaces.
    rt.text_save_as(at(""), "copy.txt".into(), "новый файл".into()).unwrap();
    assert_eq!(fs::read_to_string(ws.join("copy.txt")).unwrap(), "новый файл");
    assert!(fs::read_dir(&ws).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")),
        "successful save-as retained its spool");
    assert!(matches!(rt.text_save_as(at(""), "copy.txt".into(), "x".into()), Err(BridgeError::Conflict)));

    // Office: DOCX blocks and runs, XLSX sheet cells.
    fs::write(ws.join("doc.docx"), docx()).unwrap();
    let view = rt.office_open(at("doc.docx")).unwrap();
    assert_eq!((view.kind, view.truncated), (1, false));
    assert_eq!(view.blocks.len(), 2);
    let runs: Vec<(String, bool)> = view.blocks[0].runs.iter().map(|r| (r.text.clone(), r.bold)).collect();
    assert_eq!(runs, [("Жирный".to_string(), true), (" обычный".to_string(), false)]);
    assert_eq!((view.blocks[1].kind, view.blocks[1].rows.clone()), (2, vec![vec!["A1".to_string(), "B1".to_string()]]));
    fs::write(ws.join("book.xlsx"), xlsx()).unwrap();
    let view = rt.office_open(at("book.xlsx")).unwrap();
    assert_eq!(view.kind, 2);
    assert_eq!(view.sheets[0].name, "Лист");
    assert_eq!(view.sheets[0].rows, vec![vec!["имя".to_string(), "42".to_string()]]);
    // Not an Office file.
    assert!(rt.office_open(at("copy.txt")).is_err());
    // The intentional failed publication keeps its only prepared source for
    // inspection/recovery. Successful saves above must still clean up promptly.
    let retained: Vec<_> = fs::read_dir(&ws).unwrap().map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(".pnc-spool")).collect();
    assert_eq!(retained.len(), 1, "only the conflicted save-as should retain input");
    assert_eq!(fs::read_to_string(&retained[0]).unwrap(), "x");
    let referenced = crate::spool::referenced(&rt.core, rt.app.id()).unwrap();
    assert!(referenced.contains(retained[0].file_name().unwrap().to_str().unwrap()));
    rt.shutdown().unwrap();
}

