//! Server trash retention (owner decision 2026-11-13). DELETE moves items into
//! buckets `<unix-ms>-<uuid>` of the hidden trash; nothing is erased by a
//! client. Buckets are removed only by the server owner:
//! * automatically after `trash_days` (config; absent = keep until emptied);
//! * on demand: `paranoic-server empty-trash config.json [--older-than DAYS]`.
//!
//! There is deliberately no WebDAV request for this: whoever steals the access
//! token must not be able to empty the trash after deleting files.
use std::{path::Path, time::{Duration, SystemTime, UNIX_EPOCH}};

pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Remove trash buckets older than `older_than` (`None`: all). Entries whose
/// names are not buckets are left alone. Links are removed, never followed
/// (`remove_dir_all` does not follow them). Keeps going after an error and
/// returns the first one; otherwise the number of buckets removed.
pub fn purge(root: &Path, trash: &str, older_than: Option<Duration>) -> std::io::Result<usize> {
    let folder = root.join(trash);
    let info = std::fs::symlink_metadata(&folder)?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "trash is not a plain folder"));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let (mut removed, mut first_error) = (0usize, None);
    for entry in std::fs::read_dir(&folder)? {
        let entry = match entry { Ok(entry) => entry, Err(error) => { first_error.get_or_insert(error); continue } };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stamp) = name.split_once('-').and_then(|(ms, rest)| (!rest.is_empty()).then_some(ms)).and_then(|ms| ms.parse::<u128>().ok())
            else { continue };
        if older_than.is_some_and(|age| now.saturating_sub(stamp) < age.as_millis()) { continue; }
        let result = match entry.file_type() {
            Ok(kind) if kind.is_dir() && !kind.is_symlink() => std::fs::remove_dir_all(entry.path()),
            Ok(_) => std::fs::remove_file(entry.path()),
            Err(error) => Err(error),
        };
        match result { Ok(()) => removed += 1, Err(error) => { first_error.get_or_insert(error); } }
    }
    match first_error { Some(error) => Err(error), None => Ok(removed) }
}
