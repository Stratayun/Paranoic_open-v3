//! Plan 5.3: notes as plain `.txt` / `.md` files (owner decision R-5: open
//! export and import, no encryption; the UI warns that the copy is not
//! protected and Clear does not erase it). Pure conversion: the caller writes
//! or reads the files through Core. Attachments never leave the notes (R-3).
use crate::{Note, NotesLimits, Notebook, Result, NotesError, Tab};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

/// One exported file: `Tab/Note.md` (relative, portable names).
pub struct PlainFile { pub path: String, pub bytes: Zeroizing<Vec<u8>> }
/// `skipped_attachments`: attachments that stayed inside (R-3), for the UI.
pub struct PlainExport { pub files: Vec<PlainFile>, pub skipped_attachments: usize }
/// `skipped`: files that are not `.txt`/`.md` or not UTF-8 text.
pub struct PlainImport { pub tabs: Vec<Tab>, pub skipped: usize }

const NAME_CHARS: usize = 100;

/// A portable file or folder name from a title: no separators, reserved or
/// control characters, no trailing dots/spaces, at most 100 characters.
fn portable(title: &str, fallback: &str) -> String {
    let cleaned: String = title.chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .take(NAME_CHARS).collect();
    let mut trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    // Audit A11: the same rules as Core's names. At most 200 UTF-8 bytes (room for " (n)" and
    // ".md" within 255), and every Windows device name, also before a dot ("CON.txt", "COM2").
    while trimmed.len() > 200 { trimmed.pop(); }
    let trimmed = trimmed.trim_end_matches(['.', ' ']).to_owned();
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|p| stem.strip_prefix(p).is_some_and(|d| matches!(d, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")))
        || trimmed.to_ascii_uppercase().starts_with(".PARANOIC-");
    if trimmed.is_empty() || trimmed.starts_with('.') || reserved { fallback.to_owned() } else { trimmed }
}
/// `name`, then `name (2)`, `name (3)`… unique ignoring case.
fn unique(name: String, taken: &mut BTreeSet<String>) -> String {
    let mut candidate = name.clone();
    let mut n = 2;
    while !taken.insert(candidate.to_lowercase()) { candidate = format!("{name} ({n})"); n += 1; }
    candidate
}

/// Every tab becomes a folder, every note `<title>.md` with its text.
pub fn export_plain(notebook: &Notebook) -> PlainExport {
    let mut files = Vec::new();
    let mut skipped_attachments = 0;
    let mut folders = BTreeSet::new();
    for tab in &notebook.tabs {
        let folder = unique(portable(&tab.title, "Вкладка"), &mut folders);
        let mut names = BTreeSet::new();
        for note in &tab.notes {
            skipped_attachments += note.attachments.len();
            let name = unique(portable(&note.title, "Заметка"), &mut names);
            files.push(PlainFile { path: format!("{folder}/{name}.md"), bytes: Zeroizing::new(note.body.as_bytes().to_vec()) });
        }
    }
    PlainExport { files, skipped_attachments }
}

/// Files `(relative path, bytes)` → new tabs. A file's folder is its tab;
/// files at the top go to `root_tab`. Title = file name without extension.
/// Ids are new, so imported notes never replace existing ones.
pub fn import_plain(files: &[(String, Vec<u8>)], root_tab: &str, limits: &NotesLimits, now_ms: u64) -> Result<PlainImport> {
    let mut tabs: Vec<Tab> = Vec::new();
    let mut skipped = 0;
    let mut total = 0usize;
    for (path, bytes) in files {
        let mut parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        let Some(file) = parts.pop() else { skipped += 1; continue };
        let Some((stem, extension)) = file.rsplit_once('.') else { skipped += 1; continue };
        if !(extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("txt")) { skipped += 1; continue; }
        let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let Ok(body) = std::str::from_utf8(text) else { skipped += 1; continue };
        total = total.saturating_add(body.len());
        if total > limits.notebook_bytes { return Err(NotesError::Limit); }
        let tab_title = parts.last().copied().unwrap_or(root_tab);
        let index = match tabs.iter().position(|t| t.title == tab_title) {
            Some(i) => i,
            None => {
                if tabs.len() >= limits.tabs { return Err(NotesError::Limit); }
                tabs.push(Tab { id: uuid::Uuid::new_v4().to_string(), title: clip(tab_title, limits.title_chars), notes: Vec::new() });
                tabs.len() - 1
            }
        };
        let tab = &mut tabs[index];
        if tab.notes.len() >= limits.notes_per_tab { return Err(NotesError::Limit); }
        tab.notes.push(Note { id: uuid::Uuid::new_v4().to_string(), title: clip(stem, limits.title_chars),
            body: body.to_owned(), modified_ms: now_ms, attachments: Vec::new() });
    }
    Ok(PlainImport { tabs, skipped })
}
fn clip(text: &str, chars: usize) -> String { text.chars().take(chars).collect() }

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::Attachment;

    fn note(title: &str, body: &str, attachments: usize) -> Note {
        Note { id: title.into(), title: title.into(), body: body.into(), modified_ms: 1,
            attachments: (0..attachments).map(|i| Attachment { id: format!("a{i}"), name: "f".into(), size: 1 }).collect() }
    }

    #[test] fn export_names_are_portable_unique_and_attachments_stay_inside() {
        let book = Notebook { tabs: vec![
            Tab { id: "1".into(), title: "Work/Home".into(), notes: vec![note("Plan: Q1?", "text", 2), note("plan_ q1_", "other", 0), note("", "x", 0)] },
            Tab { id: "2".into(), title: "work_home".into(), notes: vec![note("CON", "y", 0), note("..hidden", "z", 0)] },
        ] };
        let export = export_plain(&book);
        let paths: Vec<&str> = export.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["Work_Home/Plan_ Q1_.md", "Work_Home/plan_ q1_ (2).md", "Work_Home/Заметка.md",
            "work_home (2)/Заметка.md", "work_home (2)/Заметка (2).md"]);
        assert_eq!(&export.files[0].bytes[..], b"text");
        assert_eq!(export.skipped_attachments, 2);
    }

    #[test] fn import_round_trip_folders_root_files_and_skips() {
        let book = Notebook { tabs: vec![Tab { id: "1".into(), title: "Дом".into(), notes: vec![note("Список", "молоко\nхлеб", 0)] }] };
        let mut files: Vec<(String, Vec<u8>)> = export_plain(&book).files.into_iter().map(|f| (f.path, f.bytes.to_vec())).collect();
        files.push(("loose.txt".into(), b"\xEF\xBB\xBFroot note".to_vec()));
        files.push(("photo.jpg".into(), vec![0xFF, 0xD8]));
        files.push(("bad.md".into(), vec![0xC3, 0x28]));
        let imported = import_plain(&files, "Импорт", &NotesLimits::default(), 42).unwrap();
        assert_eq!(imported.skipped, 2);
        let titles: Vec<&str> = imported.tabs.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Дом", "Импорт"]);
        assert_eq!((imported.tabs[0].notes[0].title.as_str(), imported.tabs[0].notes[0].body.as_str()), ("Список", "молоко\nхлеб"));
        assert_eq!(imported.tabs[1].notes[0].body, "root note", "BOM removed");
        assert!(imported.tabs.iter().all(|t| t.id != "1") && imported.tabs[0].notes[0].id != "Список", "new ids");
        assert_eq!(imported.tabs[0].notes[0].modified_ms, 42);
    }

    #[test] fn import_respects_limits() {
        let limits = NotesLimits { notes_per_tab: 1, ..NotesLimits::default() };
        let files = vec![("a.md".to_string(), b"1".to_vec()), ("b.md".to_string(), b"2".to_vec())];
        assert!(matches!(import_plain(&files, "T", &limits, 0), Err(NotesError::Limit)));
        let small = NotesLimits { notebook_bytes: 3, ..NotesLimits::default() };
        assert!(matches!(import_plain(&[("a.md".into(), b"1234".to_vec())], "T", &small, 0), Err(NotesError::Limit)));
    }
}

#[cfg(test)]
mod export_name_tests {
    /// Audit A11: names the exporter makes must pass Core's own name rules.
    #[test] fn device_names_and_long_titles_are_portable() {
        assert_eq!(super::portable("COM2", "x"), "x");
        assert_eq!(super::portable("CON.txt", "x"), "x");
        assert_eq!(super::portable("conin$", "x"), "x");
        assert_eq!(super::portable("Console", "x"), "Console");
        let long = super::portable(&"界".repeat(100), "x");
        assert!(long.len() <= 200 && long.chars().all(|c| c == '界'));
    }
}
