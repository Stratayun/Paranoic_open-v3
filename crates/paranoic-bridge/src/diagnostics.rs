//! Plan 3.2 (bridge part): the most recent Core errors for "Подробнее" and the
//! debug report. Only the error code and Core's fixed message are kept —
//! never paths, names, passwords or content. Memory only: gone at exit, on
//! request and with Clear.
use crate::*;
use std::collections::VecDeque;

const KEEP: usize = 100;
const MESSAGE: usize = 160;
static RECENT: Mutex<VecDeque<NativeDiagnostic>> = Mutex::new(VecDeque::new());

/// `at_ms`: Unix time; `code`: Core's error category (e.g. "Unsupported").
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NativeDiagnostic { pub at_ms: u64, pub code: String, pub message: String }

fn push(code: &str, message: &str) {
    let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let entry = NativeDiagnostic { at_ms, code: code.chars().take(32).collect(), message: message.chars().take(MESSAGE).collect() };
    let mut recent = match RECENT.lock() { Ok(g) => g, Err(e) => e.into_inner() };
    if recent.len() >= KEEP { recent.pop_front(); }
    recent.push_back(entry);
}
pub(crate) fn record(error: &paranoic_core::CoreError) { push(&format!("{:?}", error.code), &error.message) }
/// Errors that reach the host without a CoreError (format/crypto layers).
/// `CryptoError` variants carry no paths or secrets.
pub(crate) fn note(code: &str, message: &str) { push(code, message) }
pub(crate) fn clear() {
    match RECENT.lock() { Ok(mut g) => g.clear(), Err(e) => e.into_inner().clear() }
}

/// One connected place in the report: no name, no path. `kind`: "app" (the
/// app's own folder), "server" (WebDAV), "local" (folder).
#[derive(Clone, uniffi::Record)]
pub struct NativeSpaceReport {
    pub kind: String, pub reachable: bool, pub read: bool, pub write: bool, pub delete: bool,
    /// Rename without replacement and directory sync: vault writes need both.
    pub atomic_rename: bool, pub directory_sync: bool,
}
/// Plan 3.3: facts for "Диагностика". `build`: commit the binary was built
/// from (PARANOIC_BUILD_SHA at build time) or "unknown". `notes_key`: 0
/// password only, 1 device key, 2 hardware key; None if unavailable.
#[derive(Clone, uniffi::Record)]
pub struct NativeDiagnosticReport {
    pub version: String, pub build: String, pub target: String,
    pub pdfium_present: bool, pub worker_present: bool, pub worker_host: bool,
    pub notes_key: Option<u8>, pub spaces: Vec<NativeSpaceReport>, pub recent: Vec<NativeDiagnostic>,
}
fn present(path: &std::path::Path) -> bool { path.is_absolute() && std::fs::metadata(path).is_ok_and(|m| m.is_file()) }
fn yes(value: bool) -> &'static str { if value { "да" } else { "нет" } }

impl NativeRuntime {
    fn space_reports(&self) -> Vec<NativeSpaceReport> {
        let grants = match self.grants.lock() { Ok(g) => g.clone(), Err(e) => e.into_inner().clone() };
        let spaces = match self.spaces.lock() { Ok(s) => s.clone(), Err(e) => e.into_inner().clone() };
        spaces.iter().map(|space| {
            let kind = match grants.iter().find(|g| g.id == space.id) {
                None => "app",
                Some(g) if g.reference.starts_with("webdav:") => "server",
                Some(_) => "local",
            };
            let location = space.root.core().ok();
            let storage = location.as_ref().and_then(|l| self.core.storage(l.storage).ok());
            let caps = storage.as_ref().map(|s| s.capabilities()).unwrap_or_default();
            let reachable = location.is_some_and(|l| self.core.metadata(&l).is_ok());
            NativeSpaceReport { kind: kind.into(), reachable, read: caps.read, write: caps.write, delete: caps.delete,
                atomic_rename: caps.atomic_move_no_replace, directory_sync: caps.durable_directory_sync }
        }).collect()
    }
}

#[uniffi::export]
impl NativeRuntime {
    pub fn diagnostics_report(&self) -> NativeDiagnosticReport {
        let worker_host = matches!(self.document_worker_host(), Ok(Some(_)));
        NativeDiagnosticReport {
            version: env!("CARGO_PKG_VERSION").into(),
            build: option_env!("PARANOIC_BUILD_SHA").unwrap_or("unknown").into(),
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            pdfium_present: present(&self.pdfium), worker_present: present(&self.pdf_worker), worker_host,
            notes_key: self.notes_status().ok().map(|s| s.guarantee),
            spaces: self.space_reports(), recent: self.diagnostics_recent(),
        }
    }
    /// The same report as plain text, for "Скопировать" and "Сохранить".
    /// Times are Unix milliseconds; nothing identifies files or folders.
    pub fn diagnostics_report_text(&self) -> String {
        let r = self.diagnostics_report();
        let mut out = format!("Paranoic {} (сборка {}), {}\n", r.version, r.build, r.target);
        out += &format!("PDFium: {}; рабочий процесс: {}; изоляция через платформу: {}\n",
            yes(r.pdfium_present), yes(r.worker_present), yes(r.worker_host));
        out += &format!("Ключ заметок: {}\n", match r.notes_key {
            Some(0) => "только пароль", Some(1) => "ключ устройства", Some(2) => "аппаратный ключ", _ => "недоступно" });
        out += "Места:\n";
        for (i, s) in r.spaces.iter().enumerate() {
            out += &format!("  {}. {}: доступно {}, чтение {}, запись {}, удаление {}, атомарное переименование {}, синхронизация каталога {}\n",
                i + 1, s.kind, yes(s.reachable), yes(s.read), yes(s.write), yes(s.delete), yes(s.atomic_rename), yes(s.directory_sync));
        }
        out += &format!("Последние ошибки ({}):\n", r.recent.len());
        for d in r.recent.iter().rev() { out += &format!("  {} {}: {}\n", d.at_ms, d.code, d.message); }
        out
    }

    /// Oldest first, at most 100.
    pub fn diagnostics_recent(&self) -> Vec<NativeDiagnostic> {
        match RECENT.lock() { Ok(g) => g.iter().cloned().collect(), Err(e) => e.into_inner().iter().cloned().collect() }
    }
    pub fn diagnostics_clear(&self) { clear() }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test] fn keeps_code_and_message_bounded_and_clears() {
        clear();
        // Other tests record concurrently: look only for this test's marker.
        let marker = "diagnostics test marker";
        let error = paranoic_core::CoreError::new(paranoic_core::ErrorCode::Unsupported, marker);
        let _: BridgeError = error.into();
        let has = || RECENT.lock().unwrap().iter().any(|d| d.code == "Unsupported" && d.message == marker);
        assert!(has());
        for _ in 0..(KEEP + 5) { record(&paranoic_core::CoreError::new(paranoic_core::ErrorCode::Io, "x".repeat(500))); }
        let recent = RECENT.lock().unwrap();
        assert!(recent.len() <= KEEP && recent.iter().all(|d| d.message.chars().count() <= MESSAGE));
        drop(recent);
        record(&paranoic_core::CoreError::new(paranoic_core::ErrorCode::Unsupported, marker));
        clear();
        assert!(!has());
    }
}
