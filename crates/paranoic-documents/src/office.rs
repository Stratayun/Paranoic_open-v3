//! Read-only structural viewers for DOCX, XLSX and ODS (roadmap 2.6).
//!
//! Pure Rust, no macros/scripts/external resources. Packages pass the strict
//! ZIP preflight (`zip_guard`), then only XML/relationship parts are inflated
//! with exact length + CRC checks and per-part/total ceilings. XML is parsed
//! with quick-xml; DOCTYPE/DTD is rejected and only predefined/character
//! entities are resolved (no XXE, no entity expansion). Spreadsheet repetition
//! attributes and sparse cell addresses never allocate beyond the view window.
//! Formulas are not evaluated: cached values are shown. Not an editor.
use std::{borrow::Cow, collections::BTreeMap, io::{Cursor, Read, Seek}};
use paranoic_core::{Core, EntryKind, Location};
use quick_xml::{events::{BytesStart, Event}, Reader};
use zeroize::{Zeroize, Zeroizing};
use crate::{ArchiveLimits, CoreError, ErrorCode, Result};

#[derive(Clone, Copy, Debug)]
pub struct OfficeLimits {
    /// Packaged file size.
    pub file_bytes: u64,
    /// Inflated size of any single XML part.
    pub part_bytes: u64,
    /// Inflated size of all retained XML parts.
    pub total_bytes: u64,
    pub entries: usize,
    /// Output view window.
    pub rows: usize, pub columns: usize, pub sheets: usize,
    pub cell_chars: usize, pub blocks: usize,
    /// Total characters retained in the view.
    pub text_chars: usize,
    /// Materialized cell and row slots for the whole document (including the
    /// empty cells created to reach a sparse address): bounds memory of the
    /// table structures independently of the text budget.
    pub cells: usize,
    pub xml_depth: usize,
    /// Auxiliary tables are bounded independently of the materialized view.
    pub auxiliary_items: usize, pub auxiliary_bytes: usize, pub xml_events: u64,
}
impl Default for OfficeLimits {
    fn default() -> Self {
        Self { file_bytes: 256 << 20, part_bytes: 64 << 20, total_bytes: 192 << 20, entries: 10_000,
            rows: 10_000, columns: 256, sheets: 256, cell_chars: 32_768, blocks: 200_000,
            text_chars: 32 << 20, xml_depth: 256, cells: 2_000_000,
            auxiliary_items: 200_000, auxiliary_bytes: 16 << 20, xml_events: 5_000_000 }
    }
}
impl OfficeLimits {
    pub fn phone() -> Self {
        Self { file_bytes: 32 << 20, part_bytes: 8 << 20, total_bytes: 32 << 20,
            rows: 2000, columns: 128, sheets: 32, cells: 200_000, text_chars: 4 << 20,
            auxiliary_items: 50_000, auxiliary_bytes: 4 << 20, xml_events: 1_000_000, ..Self::default() }
    }
    fn validate(&self) -> Result<()> {
        if self.file_bytes == 0 || self.file_bytes > 4 << 30 || self.part_bytes == 0 || self.part_bytes > 512 << 20
            || self.total_bytes < self.part_bytes || self.total_bytes > 1 << 30 || self.entries == 0 || self.entries > 100_000
            || self.rows == 0 || self.rows > 1_048_576 || self.columns == 0 || self.columns > 16_384
            || self.rows.saturating_mul(self.columns) > 50_000_000 || self.sheets == 0 || self.sheets > 4096
            || self.cell_chars == 0 || self.cells == 0 || self.cells > 50_000_000 || self.blocks == 0 || self.text_chars == 0 || self.xml_depth == 0 || self.xml_depth > 4096
            || self.auxiliary_items == 0 || self.auxiliary_items > 2_000_000 || self.auxiliary_bytes == 0 || self.auxiliary_bytes > 128 << 20
            || self.xml_events == 0 || self.xml_events > 50_000_000 {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficeKind { Docx, Xlsx, Ods }

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Run { pub text: String, pub bold: bool, pub italic: bool, pub underline: bool }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    /// `heading` is 1..=9 for headings, 0 for Title, None for body text.
    Paragraph { style: Option<String>, heading: Option<u8>, list: bool, runs: Vec<Run> },
    Table { rows: Vec<Vec<String>> },
}

/// Plaintext strings are overwritten on drop (codec-internal copies are not).
#[derive(Debug, Default)]
pub struct WordView { pub blocks: Vec<Block>, pub truncated: bool }
#[derive(Debug, Default)]
pub struct SheetView { pub name: String, pub rows: Vec<Vec<String>>, pub truncated: bool }
#[derive(Debug, Default)]
pub struct WorkbookView { pub sheets: Vec<SheetView>, pub truncated: bool }
#[derive(Debug)]
pub enum OfficeView { Word(WordView), Workbook(WorkbookView) }

impl Drop for WordView {
    fn drop(&mut self) {
        for block in &mut self.blocks {
            match block {
                Block::Paragraph { style, runs, .. } => { if let Some(s) = style { s.zeroize(); } for r in runs { r.text.zeroize(); } }
                Block::Table { rows } => wipe_rows(rows),
            }
        }
    }
}
impl Drop for SheetView { fn drop(&mut self) { self.name.zeroize(); wipe_rows(&mut self.rows); } }
fn wipe_rows(rows: &mut [Vec<String>]) { for row in rows { for cell in row { cell.zeroize(); } } }

impl OfficeView {
    pub fn kind_name(&self) -> &'static str { match self { Self::Word(_) => "document", Self::Workbook(_) => "workbook" } }
}

/// Parse an in-memory package. Run off the UI thread.
pub fn open_office(input: impl Read + Seek, limits: OfficeLimits, mut check: impl FnMut() -> Result<()>) -> Result<OfficeView> {
    limits.validate()?;
    let package = Package::read(input, limits, &mut check)?;
    let mut budget = Budget { chars: limits.text_chars, cells: limits.cells, truncated: false };
    match package.kind()? {
        (OfficeKind::Docx, main) => docx(&package, &main, limits, &mut budget, &mut check).map(OfficeView::Word),
        (OfficeKind::Xlsx, main) => xlsx(&package, &main, limits, &mut budget, &mut check).map(OfficeView::Workbook),
        (OfficeKind::Ods, _) => ods(&package, limits, &mut budget, &mut check).map(OfficeView::Workbook),
    }
}

/// Read a granted file through Core into a zeroizing buffer, then parse it.
pub fn open_office_location(core: &Core, source: &Location, limits: OfficeLimits,
    mut check: impl FnMut() -> Result<()>) -> Result<OfficeView> {
    limits.validate()?; check()?;
    let mut input = core.open_read(source)?;
    let before = input.metadata()?;
    if before.kind != EntryKind::File || before.bytes > limits.file_bytes { return Err(large()); }
    let mut bytes = Zeroizing::new(Vec::with_capacity(before.bytes as usize));
    let mut buffer = Zeroizing::new(vec![0u8; 65536]);
    loop {
        check()?;
        let n = input.read(&mut buffer).map_err(CoreError::io)?;
        if n == 0 { break; }
        if bytes.len() as u64 + n as u64 > before.bytes { return Err(changed()); }
        bytes.extend_from_slice(&buffer[..n]);
    }
    if bytes.len() as u64 != before.bytes || input.metadata()? != before { return Err(changed()); }
    open_office(Cursor::new(&bytes[..]), limits, check)
}

// ---------------- package ----------------
struct Package { parts: BTreeMap<String, Zeroizing<Vec<u8>>>, limits: OfficeLimits,
    auxiliary: std::cell::RefCell<(usize, usize)> }
impl Package {
    fn read(mut input: impl Read + Seek, limits: OfficeLimits, check: &mut impl FnMut() -> Result<()>) -> Result<Self> {
        let archive = ArchiveLimits { archive_bytes: limits.file_bytes, entries: limits.entries, entry_bytes: limits.part_bytes.max(1),
            total_bytes: u64::MAX >> 1, directory_bytes: 32 << 20, ratio: u64::MAX, depth: 64 };
        crate::zip_guard::inspect(&mut input, archive, check)?;
        let mut zip = zip::ZipArchive::new(input).map_err(|_| invalid())?;
        let mut parts = BTreeMap::new(); let mut total = 0u64;
        for index in 0..zip.len() {
            check()?;
            let mut file = zip.by_index(index).map_err(|_| invalid())?;
            let name = std::str::from_utf8(file.name_raw()).map_err(|_| invalid())?.to_owned();
            let lower = name.to_ascii_lowercase();
            if file.is_dir() || !(lower.ends_with(".xml") || lower.ends_with(".rels") || lower == "mimetype") { continue; }
            if file.encrypted() { return Err(CoreError::new(ErrorCode::Unsupported, "encrypted office documents are not supported")); }
            let size = file.size();
            if size > limits.part_bytes { return Err(large()); }
            total = total.checked_add(size).ok_or_else(large)?;
            if total > limits.total_bytes { return Err(large()); }
            let mut data = Zeroizing::new(Vec::with_capacity(size as usize));
            let mut buffer = Zeroizing::new(vec![0u8; 65536]);
            let mut limited = (&mut file).take(size + 1);
            loop {
                check()?;
                let n = limited.read(&mut buffer).map_err(|_| invalid())?;
                if n == 0 { break; }
                data.extend_from_slice(&buffer[..n]);
            }
            // Reading to EOF lets the codec verify CRC; length must be exact.
            if data.len() as u64 != size { return Err(invalid()); }
            if parts.insert(name, data).is_some() { return Err(invalid()); }
        }
        Ok(Self { parts, limits, auxiliary: std::cell::RefCell::new((limits.auxiliary_items, limits.auxiliary_bytes)) })
    }
    fn reserve_auxiliary(&self, items: usize, bytes: usize) -> Result<()> {
        let mut remaining = self.auxiliary.borrow_mut();
        if items > remaining.0 || bytes > remaining.1 { return Err(large()); }
        remaining.0 -= items; remaining.1 -= bytes; Ok(())
    }
    fn part(&self, name: &str) -> Result<&[u8]> {
        self.parts.get(name).map(|p| &p[..]).ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "office part missing"))
    }
    fn kind(&self) -> Result<(OfficeKind, String)> {
        if let Some(mime) = self.parts.get("mimetype") {
            return match mime.trim_ascii() {
                b"application/vnd.oasis.opendocument.spreadsheet" => Ok((OfficeKind::Ods, "content.xml".into())),
                _ => Err(unsupported()),
            };
        }
        // OOXML: main part from the package relationships, type from [Content_Types].xml.
        let main = relationships(self, self.part("_rels/.rels")?, "")?.into_iter()
            .find(|(_, kind, _)| kind.ends_with("/officeDocument")).map(|(_, _, target)| target).ok_or_else(unsupported)?;
        let mut types = Xml::new(self.part("[Content_Types].xml")?, 64, self.limits.xml_events)?;
        let mut content = None; let wanted = format!("/{main}");
        while let Some(event) = types.next()? {
            if let Event::Start(e) | Event::Empty(e) = &event {
                if e.local_name().as_ref() == b"Override" && attr(e, b"PartName")?.as_deref() == Some(wanted.as_str()) {
                    content = attr(e, b"ContentType")?.map(Cow::into_owned);
                }
            }
        }
        let content = content.ok_or_else(unsupported)?;
        if content.contains("wordprocessingml.document.main") { Ok((OfficeKind::Docx, main)) }
        else if content.contains("spreadsheetml.sheet.main") { Ok((OfficeKind::Xlsx, main)) }
        else { Err(unsupported()) }
    }
}

/// (Id, Type, resolved target) of a relationships part; external targets skipped.
fn relationships(package: &Package, part: &[u8], base: &str) -> Result<Vec<(String, String, String)>> {
    let mut xml = Xml::new(part, 64, package.limits.xml_events)?; let mut result = Vec::new();
    while let Some(event) = xml.next()? {
        if let Event::Start(e) | Event::Empty(e) = &event {
            if e.local_name().as_ref() != b"Relationship" { continue; }
            if attr(e, b"TargetMode")?.as_deref() == Some("External") { continue; }
            let (Some(id), Some(kind), Some(target)) = (attr(e, b"Id")?, attr(e, b"Type")?, attr(e, b"Target")?) else { return Err(invalid()); };
            let resolved = resolve(base, &target)?;
            package.reserve_auxiliary(1, id.len().saturating_add(kind.len()).saturating_add(resolved.len()))?;
            result.push((id.into_owned(), kind.into_owned(), resolved));
        }
    }
    Ok(result)
}
fn resolve(base: &str, target: &str) -> Result<String> {
    let joined = if let Some(absolute) = target.strip_prefix('/') { absolute.to_owned() } else if base.is_empty() { target.to_owned() } else { format!("{base}/{target}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part { "" | "." => {}, ".." => { parts.pop().ok_or_else(invalid)?; } other => parts.push(other) }
    }
    Ok(parts.join("/"))
}
fn directory(path: &str) -> &str { path.rsplit_once('/').map(|(d, _)| d).unwrap_or("") }
fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') { Some((d, f)) => format!("{d}/_rels/{f}.rels"), None => format!("_rels/{part}.rels") }
}

// ---------------- XML ----------------
struct Xml<'a> { reader: Reader<&'a [u8]>, depth: usize, max_depth: usize, events: u64, max_events: u64 }
impl<'a> Xml<'a> {
    fn new(bytes: &'a [u8], max_depth: usize, max_events: u64) -> Result<Self> {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        if std::str::from_utf8(bytes).is_err() { return Err(CoreError::new(ErrorCode::Unsupported, "only UTF-8 XML parts are supported")); }
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().check_end_names = true;
        Ok(Self { reader, depth: 0, max_depth, events: 0, max_events })
    }
    fn next(&mut self) -> Result<Option<Event<'a>>> {
        loop {
            let event = self.reader.read_event().map_err(|_| invalid())?;
            if self.events >= self.max_events { return Err(large()); }
            self.events += 1;
            match &event {
                Event::Eof => { return if self.depth == 0 { Ok(None) } else { Err(invalid()) }; }
                Event::DocType(_) => return Err(CoreError::new(ErrorCode::Unsupported, "XML DTD is not accepted")),
                Event::Start(_) => { self.depth += 1; if self.depth > self.max_depth { return Err(large()); } }
                Event::End(_) => { self.depth = self.depth.checked_sub(1).ok_or_else(invalid)?; }
                Event::Comment(_) | Event::PI(_) | Event::Decl(_) => continue,
                _ => {}
            }
            return Ok(Some(event));
        }
    }
    fn tick(&self) -> bool { self.events.is_multiple_of(4096) }
}
/// Text of a Text/CData/GeneralRef event; unknown named entities are rejected.
fn text<'b>(event: &'b Event<'_>) -> Result<Option<Cow<'b, str>>> {
    Ok(match event {
        Event::Text(t) => Some(t.xml10_content().map_err(|_| invalid())?),
        Event::CData(t) => Some(t.decode().map_err(|_| invalid())?),
        Event::GeneralRef(r) => {
            if let Some(c) = r.resolve_char_ref().map_err(|_| invalid())? { Some(Cow::Owned(c.to_string())) } else {
                Some(Cow::Owned(match r.decode().map_err(|_| invalid())?.as_ref() {
                    "amp" => "&", "lt" => "<", "gt" => ">", "quot" => "\"", "apos" => "'", _ => return Err(invalid()),
                }.to_owned()))
            }
        }
        _ => None,
    })
}
fn attr<'b>(e: &'b BytesStart<'_>, local: &[u8]) -> Result<Option<Cow<'b, str>>> {
    for a in e.attributes() {
        let a = a.map_err(|_| invalid())?;
        if a.value.len() > 65_536 { return Err(large()); }
        if a.key.local_name().as_ref() == local { return Ok(Some(a.unescape_value().map_err(|_| invalid())?)); }
    }
    Ok(None)
}
fn truthy(e: &BytesStart<'_>) -> Result<bool> {
    Ok(!matches!(attr(e, b"val")?.as_deref(), Some("0" | "false" | "none" | "off")))
}

struct Budget { chars: usize, cells: usize, truncated: bool }
impl Budget {
    /// Reserve `slots` structure entries; false (and truncated) when exhausted.
    fn reserve(&mut self, slots: usize) -> bool {
        if slots > self.cells { self.truncated = true; self.cells = 0; return false; }
        self.cells -= slots; true
    }
    /// Append at most the remaining budget and per-item limit (UTF-8 bytes),
    /// cutting on a character boundary.
    fn push(&mut self, target: &mut String, value: &str, item_limit: usize) {
        let room = self.chars.min(item_limit.saturating_sub(target.len()));
        let mut end = value.len().min(room);
        while !value.is_char_boundary(end) { end -= 1; }
        if end < value.len() { self.truncated = true; }
        target.push_str(&value[..end]); self.chars -= end;
    }
}

/// Bounded UTF-8 append, O(incoming bytes) without rescanning the growing text.
fn bounded_push(target: &mut String, value: &str, limit: usize) -> bool {
    let mut end = value.len().min(limit.saturating_sub(target.len()));
    while !value.is_char_boundary(end) { end -= 1; }
    target.push_str(&value[..end]); end < value.len()
}

// ---------------- DOCX ----------------
fn docx(package: &Package, main: &str, limits: OfficeLimits, budget: &mut Budget, check: &mut impl FnMut() -> Result<()>) -> Result<WordView> {
    let base = directory(main);
    // Style id -> heading level (from style names "heading N" / "Title").
    let mut headings: BTreeMap<String, u8> = BTreeMap::new();
    // Style id -> numbering in its paragraph properties (false: numId 0 = "no
    // list"), and style id -> basedOn parent. Word's list styles keep numPr here.
    let mut style_lists: BTreeMap<String, bool> = BTreeMap::new();
    let mut based_on: BTreeMap<String, String> = BTreeMap::new();
    let styles_part = relationships(package, package.part(&rels_path(main)).unwrap_or(&b"<r/>"[..]), base)
        ?.into_iter().find(|(_, kind, _)| kind.ends_with("/styles")).map(|(_, _, t)| t);
    if let Some(Ok(bytes)) = styles_part.as_deref().map(|p| package.part(p)) {
        let mut xml = Xml::new(bytes, limits.xml_depth, limits.xml_events)?; let mut current: Option<String> = None;
        let mut in_ppr = false;
        while let Some(event) = xml.next()? {
            if xml.tick() { check()?; }
            if let Event::End(e) = &event {
                match e.local_name().as_ref() { b"pPr" => in_ppr = false, b"style" => current = None, _ => {} }
            }
            if let Event::Start(e) | Event::Empty(e) = &event {
                match e.local_name().as_ref() {
                    b"style" => {
                        let id = attr(e, b"styleId")?;
                        package.reserve_auxiliary(1, id.as_ref().map_or(0, |s| s.len().saturating_mul(4)))?;
                        current = id.map(Cow::into_owned); in_ppr = false;
                    }
                    b"pPr" if current.is_some() => in_ppr = matches!(event, Event::Start(_)),
                    b"numPr" if in_ppr => if let Some(id) = &current { style_lists.entry(id.clone()).or_insert(true); },
                    b"numId" if in_ppr => if let Some(id) = &current {
                        style_lists.insert(id.clone(), attr(e, b"val")?.is_none_or(|v| v.as_ref() != "0"));
                    },
                    b"basedOn" => if let (Some(id), Some(parent)) = (&current, attr(e, b"val")?) {
                        package.reserve_auxiliary(1, id.len().saturating_add(parent.len()))?;
                        based_on.insert(id.clone(), parent.into_owned());
                    },
                    b"name" => if let (Some(id), Some(name)) = (&current, attr(e, b"val")?) {
                        let name = name.to_ascii_lowercase();
                        let level = if name == "title" { Some(0) } else { name.strip_prefix("heading ").and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=9).contains(n)) };
                        if let Some(level) = level { headings.insert(id.clone(), level); }
                    },
                    _ => {}
                }
            }
        }
    }
    let mut view = WordView::default();
    let mut xml = Xml::new(package.part(main)?, limits.xml_depth, limits.xml_events)?;
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let (mut tables, mut skip) = (0usize, 0usize);
    let mut table: Vec<Vec<String>> = Vec::new(); let mut row: Vec<String> = Vec::new(); let mut cell = String::new();
    let mut cell_paragraphs = 0usize;
    let (mut style, mut list, mut runs): (Option<String>, Option<bool>, Vec<Run>) = (None, None, Vec::new());
    // Numbering inherited through the style chain (bounded against cycles).
    let style_list = |id: &str| -> bool {
        let mut current = id;
        for _ in 0..16 {
            if let Some(list) = style_lists.get(current) { return *list; }
            match based_on.get(current) { Some(parent) => current = parent, None => return false }
        }
        false
    };
    let mut run = Run::default(); let mut in_text = false;
    let push_text = |value: &str, tables: usize, run: &mut Run, cell: &mut String, budget: &mut Budget| {
        if tables > 0 { budget.push(cell, value, limits.cell_chars) } else { budget.push(&mut run.text, value, usize::MAX) }
    };
    while let Some(event) = xml.next()? {
        if xml.tick() { check()?; }
        if view.blocks.len() >= limits.blocks { view.truncated = true; break; }
        let (start, empty) = match &event { Event::Start(e) => (Some(e), false), Event::Empty(e) => (Some(e), true), _ => (None, false) };
        if let Some(e) = start {
            let name = e.local_name().as_ref().to_vec();
            let parent = stack.last().map(|p| p.as_slice());
            if skip > 0 || matches!(name.as_slice(), b"del" | b"delText" | b"instrText" | b"fldData" | b"drawing" | b"pict" | b"object") {
                if !empty { skip += 1; stack.push(name); } continue;
            }
            match name.as_slice() {
                b"tbl" => { tables += 1; if tables == 1 { table = Vec::new(); } }
                b"tr" if tables == 1 => row = Vec::new(),
                b"tc" if tables == 1 => { cell = String::new(); cell_paragraphs = 0; }
                b"p" => if tables == 0 { style = None; list = None; runs = Vec::new(); } else {
                    if cell_paragraphs > 0 { budget.push(&mut cell, "\n", limits.cell_chars); } cell_paragraphs += 1; },
                b"pStyle" if tables == 0 => style = attr(e, b"val")?.map(Cow::into_owned),
                b"numPr" if tables == 0 => list = Some(true),
                b"numId" if tables == 0 && parent == Some(b"numPr") => list = Some(attr(e, b"val")?.is_none_or(|v| v.as_ref() != "0")),
                b"r" => run = Run::default(),
                b"b" if parent == Some(b"rPr") => run.bold = truthy(e)?,
                b"i" if parent == Some(b"rPr") => run.italic = truthy(e)?,
                b"u" if parent == Some(b"rPr") => run.underline = truthy(e)?,
                b"t" => in_text = !empty,
                b"tab" if parent == Some(b"r") => push_text("\t", tables, &mut run, &mut cell, budget),
                b"br" | b"cr" => push_text("\n", tables, &mut run, &mut cell, budget),
                _ => {}
            }
            if !empty { stack.push(name); continue; }
        }
        let closing: Option<Vec<u8>> = match &event {
            Event::End(_) => stack.pop(),
            Event::Empty(e) => Some(e.local_name().as_ref().to_vec()),
            _ => None,
        };
        if let Some(name) = closing {
            if skip > 0 { if matches!(event, Event::End(_)) { skip -= 1; } continue; }
            match name.as_slice() {
                b"t" => in_text = false,
                b"r" if tables == 0 => {
                    let finished = std::mem::take(&mut run);
                    if finished.text.is_empty() { continue; }
                    match runs.last_mut() {
                        Some(last) if (last.bold, last.italic, last.underline) == (finished.bold, finished.italic, finished.underline) => last.text.push_str(&finished.text),
                        _ => runs.push(finished),
                    }
                }
                b"p" if tables == 0 => {
                    let heading = style.as_ref().and_then(|s| headings.get(s).copied());
                    let list = list.take().unwrap_or_else(|| style.as_deref().is_some_and(style_list));
                    view.blocks.push(Block::Paragraph { style: style.take(), heading, list, runs: std::mem::take(&mut runs) });
                }
                b"tc" if tables == 1 => { if row.len() < limits.columns && budget.reserve(1) { row.push(std::mem::take(&mut cell)); } else { view.truncated = true; } }
                b"tr" if tables == 1 => { if table.len() < limits.rows && budget.reserve(1) { table.push(std::mem::take(&mut row)); } else { view.truncated = true; } }
                b"tbl" => { tables = tables.saturating_sub(1); if tables == 0 { view.blocks.push(Block::Table { rows: std::mem::take(&mut table) }); } }
                _ => {}
            }
            continue;
        }
        if in_text && skip == 0 {
            if let Some(value) = text(&event)? { push_text(&value, tables, &mut run, &mut cell, budget); }
        }
        if budget.chars == 0 { view.truncated = true; break; }
    }
    view.truncated |= budget.truncated;
    check()?; Ok(view)
}

// ---------------- spreadsheets ----------------
struct Grid { rows: Vec<Vec<String>>, limits: OfficeLimits, truncated: bool }
impl Grid {
    fn new(limits: OfficeLimits) -> Self { Self { rows: Vec::new(), limits, truncated: false } }
    fn put(&mut self, row: u64, column: u64, value: &str, budget: &mut Budget) {
        if value.is_empty() { return; }
        if row >= self.limits.rows as u64 || column >= self.limits.columns as u64 { self.truncated = true; return; }
        let (r, c) = (row as usize, column as usize);
        let new_rows = (r + 1).saturating_sub(self.rows.len());
        let new_cells = (c + 1).saturating_sub(self.rows.get(r).map_or(0, Vec::len));
        if !budget.reserve(new_rows + new_cells) { self.truncated = true; return; }
        if self.rows.len() <= r { self.rows.resize_with(r + 1, Vec::new); }
        let line = &mut self.rows[r];
        if line.len() <= c { line.resize_with(c + 1, String::new); }
        line[c].zeroize(); line[c].clear();
        budget.push(&mut line[c], value, self.limits.cell_chars);
    }
}

fn xlsx(package: &Package, main: &str, limits: OfficeLimits, budget: &mut Budget, check: &mut impl FnMut() -> Result<()>) -> Result<WorkbookView> {
    let base = directory(main);
    let rels = relationships(package, package.part(&rels_path(main))?, base)?;
    let target = |suffix: &str| rels.iter().find(|(_, kind, _)| kind.ends_with(suffix)).map(|(_, _, t)| t.clone());
    // Shared strings.
    let mut shared: Vec<Zeroizing<String>> = Vec::new();
    if let Some(path) = target("/sharedStrings") {
        let mut xml = Xml::new(package.part(&path)?, limits.xml_depth, limits.xml_events)?;
        let mut current = Zeroizing::new(String::new()); let (mut in_t, mut phonetic) = (false, 0usize);
        while let Some(event) = xml.next()? {
            if xml.tick() { check()?; }
            match &event {
                Event::Start(e) => match e.local_name().as_ref() { b"si" => current = Zeroizing::new(String::new()), b"t" => in_t = true, b"rPh" => phonetic += 1, _ => {} },
                Event::Empty(e) if e.local_name().as_ref() == b"si" => {
                    package.reserve_auxiliary(1, 0)?; shared.push(Zeroizing::new(String::new()));
                }
                Event::End(e) => match e.local_name().as_ref() {
                    b"si" => {
                        package.reserve_auxiliary(1, 0)?;
                        shared.push(std::mem::replace(&mut current, Zeroizing::new(String::new())));
                    }
                    b"t" => in_t = false, b"rPh" => phonetic = phonetic.saturating_sub(1), _ => {} },
                other => if in_t && phonetic == 0 {
                    if let Some(value) = text(other)? {
                        let room = limits.cell_chars.saturating_sub(current.len());
                        package.reserve_auxiliary(0, value.len().min(room))?;
                        budget.truncated |= bounded_push(&mut current, &value, limits.cell_chars);
                    }
                },
            }
        }
    }
    // Styles: cell format index -> date/time kind.
    let mut date_styles: Vec<u8> = Vec::new(); // 0 none, 1 date, 2 time, 3 date+time
    if let Some(path) = target("/styles") {
        let mut xml = Xml::new(package.part(&path)?, limits.xml_depth, limits.xml_events)?;
        let mut custom: BTreeMap<u32, u8> = BTreeMap::new(); let mut in_xfs = false;
        while let Some(event) = xml.next()? {
            if xml.tick() { check()?; }
            match &event {
                Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                    b"numFmt" => if let (Some(id), Some(code)) = (attr(e, b"numFmtId")?, attr(e, b"formatCode")?) {
                        if let Ok(id) = id.parse() {
                            package.reserve_auxiliary(1, 0)?; custom.insert(id, format_kind(&code));
                        } },
                    b"cellXfs" => in_xfs = matches!(event, Event::Start(_)),
                    b"xf" if in_xfs => {
                        let id: u32 = attr(e, b"numFmtId")?.and_then(|v| v.parse().ok()).unwrap_or(0);
                        if date_styles.len() < 65_536 {
                            package.reserve_auxiliary(1, 1)?;
                            date_styles.push(custom.get(&id).copied().unwrap_or(builtin_kind(id)));
                        }
                    }
                    _ => {}
                },
                Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_xfs = false,
                _ => {}
            }
        }
    }
    // Workbook: sheet order, names, relationship ids, 1904 date system.
    let mut sheets: Vec<(String, String)> = Vec::new(); let mut date1904 = false;
    let mut sheets_truncated = false;
    let mut xml = Xml::new(package.part(main)?, limits.xml_depth, limits.xml_events)?;
    while let Some(event) = xml.next()? {
        if xml.tick() { check()?; }
        if let Event::Start(e) | Event::Empty(e) = &event {
            match e.local_name().as_ref() {
                b"workbookPr" => date1904 = matches!(attr(e, b"date1904")?.as_deref(), Some("1" | "true")),
                b"sheet" => {
                    if sheets.len() >= limits.sheets { sheets_truncated = true; continue; }
                    let (Some(name), Some(id)) = (attr(e, b"name")?, attr(e, b"id")?) else { return Err(invalid()); };
                    package.reserve_auxiliary(1, name.len().saturating_add(id.len()))?;
                    sheets.push((name.into_owned(), id.into_owned()));
                }
                _ => {}
            }
        }
    }
    let mut view = WorkbookView { truncated: sheets_truncated, ..WorkbookView::default() };
    for (name, id) in sheets {
        check()?;
        if view.sheets.len() >= limits.sheets { view.truncated = true; break; }
        let Some((_, kind, path)) = rels.iter().find(|(rid, _, _)| *rid == id) else { return Err(invalid()); };
        if !kind.ends_with("/worksheet") { continue; } // chartsheets/dialogs have no cells
        let mut grid = Grid::new(limits);
        let mut xml = Xml::new(package.part(path)?, limits.xml_depth, limits.xml_events)?;
        let (mut row_index, mut column_index) = (0u64, 0u64);
        let mut cell: Option<(u64, u64, String, usize)> = None; // row, col, type, style
        let mut value = Zeroizing::new(String::new()); let (mut in_v, mut in_is_t) = (false, false);
        while let Some(event) = xml.next()? {
            if xml.tick() { check()?; }
            match &event {
                Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                    b"row" => {
                        row_index = match attr(e, b"r")? { Some(r) => r.parse::<u64>().map_err(|_| invalid())?.checked_sub(1).ok_or_else(invalid)?, None => row_index };
                        if row_index >= 1_048_576 { return Err(invalid()); }
                        column_index = 0;
                        if matches!(event, Event::Empty(_)) { row_index = row_index.checked_add(1).ok_or_else(invalid)?; }
                    }
                    b"c" => {
                        let (r, c) = match attr(e, b"r")? { Some(reference) => cell_ref(&reference)?, None => (row_index, column_index) };
                        let kind = attr(e, b"t")?.map(Cow::into_owned).unwrap_or_else(|| "n".into());
                        let style = attr(e, b"s")?.and_then(|s| s.parse().ok()).unwrap_or(0usize);
                        column_index = c.saturating_add(1);
                        value = Zeroizing::new(String::new());
                        if matches!(event, Event::Start(_)) { cell = Some((r, c, kind, style)); }
                    }
                    b"v" => in_v = matches!(event, Event::Start(_)),
                    b"t" if cell.is_some() => in_is_t = matches!(event, Event::Start(_)),
                    _ => {}
                },
                Event::End(e) => match e.local_name().as_ref() {
                    b"v" => in_v = false, b"t" => in_is_t = false,
                    b"row" => row_index = row_index.saturating_add(1),
                    b"c" => if let Some((r, c, kind, style)) = cell.take() {
                        let shown = match kind.as_str() {
                            "s" => value.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).map(|s| s.as_str().to_owned()).unwrap_or_default(),
                            "b" => if value.trim() == "1" { "TRUE".into() } else { "FALSE".into() },
                            "n" => number(&value, date_styles.get(style).copied().unwrap_or(0), date1904),
                            _ => value.as_str().to_owned(),
                        };
                        let shown = Zeroizing::new(shown);
                        grid.put(r, c, &shown, budget);
                    },
                    _ => {}
                },
                other => if (in_v || in_is_t) && cell.is_some() {
                    if let Some(t) = text(other)? { budget.truncated |= bounded_push(&mut value, &t, limits.cell_chars); }
                },
            }
        }
        view.truncated |= grid.truncated;
        view.sheets.push(SheetView { name, rows: std::mem::take(&mut grid.rows), truncated: grid.truncated });
    }
    view.truncated |= budget.truncated;
    Ok(view)
}
fn cell_ref(reference: &str) -> Result<(u64, u64)> {
    let split = reference.find(|c: char| c.is_ascii_digit()).ok_or_else(invalid)?;
    let (letters, digits) = reference.split_at(split);
    if letters.is_empty() || letters.len() > 3 { return Err(invalid()); }
    let mut column = 0u64;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() { return Err(invalid()); }
        column = column * 26 + (c.to_ascii_uppercase() as u64 - 'A' as u64 + 1);
    }
    let row: u64 = digits.parse().map_err(|_| invalid())?;
    Ok((row.checked_sub(1).ok_or_else(invalid)?, column - 1))
}
fn builtin_kind(id: u32) -> u8 { match id { 14..=17 | 27..=36 | 50..=58 => 1, 18..=21 | 45..=47 => 2, 22 => 3, _ => 0 } }
fn format_kind(code: &str) -> u8 {
    let mut plain = String::new(); let (mut quoted, mut bracket) = (false, false);
    for c in code.chars() {
        match c { '"' => quoted = !quoted, '[' if !quoted => bracket = true, ']' if !quoted => bracket = false,
            _ if !quoted && !bracket => plain.push(c.to_ascii_lowercase()), _ => {} }
    }
    let plain = plain.split(';').next().unwrap_or("").to_owned();
    let date = plain.contains('y') || plain.contains('d');
    let time = plain.contains('h') || plain.contains('s');
    match (date, time) { (true, true) => 3, (true, false) => 1, (false, true) => 2, _ => 0 }
}
fn number(raw: &str, kind: u8, date1904: bool) -> String {
    let raw = raw.trim();
    let Ok(value) = raw.parse::<f64>() else { return raw.to_owned(); };
    if kind != 0 && value.is_finite() && (0.0..2_958_466.0).contains(&value) {
        let mut days = value.floor() as i64;
        let mut seconds = ((value - value.floor()) * 86_400.0).round() as i64;
        if seconds >= 86_400 { days += 1; seconds -= 86_400; }
        // 1900 system: serial 1 = 1900-01-01; 60 is the fictitious 1900-02-29.
        let unix_days = if date1904 { days - 24_107 } else if days < 61 { days - 25_568 } else { days - 25_569 };
        let date = civil(unix_days);
        let time = format!("{:02}:{:02}:{:02}", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60);
        return match kind { 1 => date, 2 => time, _ => format!("{date} {time}") };
    }
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{value}") }
}
/// Days since 1970-01-01 -> ISO date (proleptic Gregorian).
fn civil(days: i64) -> String {
    let z = days + 719_468; let era = z.div_euclid(146_097); let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

fn ods(package: &Package, limits: OfficeLimits, budget: &mut Budget, check: &mut impl FnMut() -> Result<()>) -> Result<WorkbookView> {
    let mut view = WorkbookView::default();
    let mut xml = Xml::new(package.part("content.xml")?, limits.xml_depth, limits.xml_events)?;
    let mut grid: Option<(String, Grid)> = None;
    let (mut row, mut column) = (0u64, 0u64);
    let mut row_repeat = 1u64; let mut row_cells: Vec<(u64, u64, Zeroizing<String>)> = Vec::new();
    let mut cell: Option<(u64, Zeroizing<String>, Option<String>, usize)> = None; // repeat, text, fallback, paragraphs
    let mut annotation = 0usize;
    while let Some(event) = xml.next()? {
        if xml.tick() { check()?; }
        let empty = matches!(event, Event::Empty(_));
        match &event {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"table" => {
                    if view.sheets.len() + usize::from(grid.is_some()) >= limits.sheets { view.truncated = true; break; }
                    let name = attr(e, b"name")?.map(Cow::into_owned).unwrap_or_default();
                    grid = Some((name, Grid::new(limits))); row = 0;
                    if empty { if let Some((name, g)) = grid.take() { view.sheets.push(SheetView { name, truncated: g.truncated, rows: g.rows }); } }
                }
                b"table-row" => {
                    row_repeat = repeat(e, b"number-rows-repeated")?; column = 0; row_cells.clear();
                    if empty { row = row.saturating_add(row_repeat); }
                }
                b"table-cell" | b"covered-table-cell" => {
                    let repeat = repeat(e, b"number-columns-repeated")?;
                    let fallback = match attr(e, b"value-type")?.as_deref() {
                        Some("date") => attr(e, b"date-value")?.map(Cow::into_owned),
                        Some("time") => attr(e, b"time-value")?.map(Cow::into_owned),
                        Some("boolean") => attr(e, b"boolean-value")?.map(|v| v.to_uppercase()),
                        Some(_) => attr(e, b"value")?.map(Cow::into_owned),
                        None => None,
                    };
                    if empty {
                        if let Some(value) = fallback { ods_cell(&mut row_cells, row, column, repeat, Zeroizing::new(value), limits, budget)?; }
                        column = column.saturating_add(repeat);
                    } else { cell = Some((repeat, Zeroizing::new(String::new()), fallback, 0)); }
                }
                b"annotation" => if !empty { annotation += 1 },
                b"p" | b"h" if annotation == 0 => if let Some((_, text, _, paragraphs)) = cell.as_mut() {
                    if *paragraphs > 0 { budget.truncated |= bounded_push(text, "\n", limits.cell_chars); } *paragraphs += 1;
                },
                b"s" if annotation == 0 => if let Some((_, text, _, _)) = cell.as_mut() {
                    let n = attr(e, b"c")?.and_then(|c| c.parse::<usize>().ok()).unwrap_or(1).min(limits.cell_chars);
                    let count = n.min(limits.cell_chars.saturating_sub(text.len()));
                    budget.truncated |= count < n;
                    text.push_str(&" ".repeat(count));
                },
                b"tab" if annotation == 0 => if let Some((_, text, _, _)) = cell.as_mut() { budget.truncated |= bounded_push(text, "\t", limits.cell_chars); },
                b"line-break" if annotation == 0 => if let Some((_, text, _, _)) = cell.as_mut() { budget.truncated |= bounded_push(text, "\n", limits.cell_chars); },
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"annotation" => annotation = annotation.saturating_sub(1),
                b"table-cell" | b"covered-table-cell" => if let Some((repeat, text, fallback, _)) = cell.take() {
                    let shown = if text.is_empty() { fallback.map(Zeroizing::new) } else { Some(text) };
                    if let Some(value) = shown { if !value.is_empty() { ods_cell(&mut row_cells, row, column, repeat, value, limits, budget)?; } }
                    column = column.saturating_add(repeat);
                },
                b"table-row" => {
                    if let Some((_, g)) = grid.as_mut() {
                        // Materialize repeated rows/columns only inside the view window.
                        if !row_cells.is_empty() {
                            let rows = row_repeat.min((limits.rows as u64).saturating_sub(row));
                            if rows < row_repeat { g.truncated = true; }
                            // Audit A21: stop expanding as soon as the cell budget is spent; slots
                            // that can no longer be stored are not walked.
                            'rows: for r in 0..rows {
                                for (start, count, value) in &row_cells {
                                    let columns = (*count).min((limits.columns as u64).saturating_sub(*start));
                                    if columns < *count { g.truncated = true; }
                                    for c in 0..columns {
                                        if budget.cells == 0 { g.truncated = true; budget.truncated = true; break 'rows; }
                                        g.put(row + r, start + c, value, budget);
                                    }
                                }
                            }
                        }
                    }
                    row = row.saturating_add(row_repeat); row_cells.clear();
                }
                b"table" => if let Some((name, g)) = grid.take() {
                    view.truncated |= g.truncated;
                    view.sheets.push(SheetView { name, truncated: g.truncated, rows: g.rows });
                },
                _ => {}
            },
            other => if annotation == 0 {
                if let Some((_, buffer, _, _)) = cell.as_mut() {
                    if let Some(value) = text(other)? { budget.truncated |= bounded_push(buffer, &value, limits.cell_chars); }
                }
            },
        }
    }
    view.truncated |= budget.truncated;
    check()?; Ok(view)
}
#[allow(clippy::too_many_arguments)]
fn ods_cell(cells: &mut Vec<(u64, u64, Zeroizing<String>)>, row: u64, column: u64, repeat: u64,
    value: Zeroizing<String>, limits: OfficeLimits, budget: &mut Budget) -> Result<()> {
    if row >= limits.rows as u64 || column >= limits.columns as u64 { budget.truncated = true; return Ok(()); }
    if cells.len() >= limits.columns { return Err(large()); }
    let mut kept = Zeroizing::new(String::new());
    budget.truncated |= bounded_push(&mut kept, &value, limits.cell_chars);
    cells.push((column, repeat, kept)); Ok(())
}
fn repeat(e: &BytesStart<'_>, name: &[u8]) -> Result<u64> {
    match attr(e, name)? { Some(v) => v.parse::<u64>().ok().filter(|n| (1..=1 << 31).contains(n)).ok_or_else(invalid), None => Ok(1) }
}

fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid or unsupported office document") }
fn unsupported() -> CoreError { CoreError::new(ErrorCode::Unsupported, "office document type is not supported") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "office document exceeds configured limits") }
fn changed() -> CoreError { CoreError::new(ErrorCode::SourceChanged, "office document changed while reading") }
