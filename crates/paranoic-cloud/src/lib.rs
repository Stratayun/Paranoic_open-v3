//! Remote storage over WebDAV (roadmap block 3): the owner's own
//! `paranoic-server` and other WebDAV servers. Google Drive, OneDrive and
//! pCloud adapters with OAuth were removed by the owner's decision (25.11.2026);
//! they remain in git history.
//!
//! Blocking HTTP (rustls); call off the UI thread. Rules shared by adapters:
//! * create-new only: uploads and folder creation never replace an entry;
//! * no automatic replay of a write whose outcome is unknown (NeedsAttention);
//! * bearer tokens are sent only to the provider's API host, never to
//!   pre-authenticated upload/download URLs or other hosts;
//! * folders are trashed only when empty; nothing is deleted permanently.
use std::io::Read;
use serde::{Deserialize, Serialize};
pub use paranoic_core::{CoreError, ErrorCode, Result};

mod http;
mod webdav;
mod transfer;
mod storage;
mod sync;
mod vault;
mod mapped;

pub use http::Http;
pub use webdav::{WebDav, WebDavCredentials};
pub use transfer::{download_to_storage, upload_from_core, verify_remote};
pub use storage::{CloudStorage, purge_spool_dir, set_spool_limit};
pub use sync::{ConflictChoice, SyncAction, SyncActionKind, SyncBaseline, SyncMode, SyncOptions, SyncPlan, SyncReport};
pub use vault::check_vault_support;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind { WebDav }

/// Provider-native object identifier. Opaque; never a path.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CloudId(String);
impl CloudId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 512 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid cloud object id"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl TryFrom<String> for CloudId { type Error = CoreError; fn try_from(v: String) -> Result<Self> { Self::parse(v) } }
impl From<CloudId> for String { fn from(v: CloudId) -> Self { v.0 } }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudKind { File, Folder, /** Provider-native document without bytes (e.g. Google Docs). */ Native }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CloudEntry {
    pub id: CloudId,
    pub name: String,
    pub kind: CloudKind,
    pub bytes: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    /// Change token (Drive version, OneDrive eTag, pCloud hash/modified).
    pub revision: Option<String>,
    /// Server-provided SHA-256 when the provider reports one.
    pub sha256: Option<[u8; 32]>,
    pub parents: Vec<CloudId>,
}

#[derive(Clone, Debug, Default)]
pub struct CloudPage { pub entries: Vec<CloudEntry>, pub next: Option<String> }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Quota { pub used: Option<u64>, pub total: Option<u64> }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CloudCapabilities {
    pub resumable_upload: bool,
    pub ranged_download: bool,
    /// Mutations can be conditioned on `revision` by the server (If-Match).
    /// Otherwise adapters re-check before mutating (residual race).
    pub conditional_mutation: bool,
    /// Server rejects a name collision atomically. Otherwise adapters
    /// check first and verify afterwards (residual race, documented).
    pub atomic_create_new: bool,
    pub duplicate_names: bool,
    pub server_sha256: bool,
    pub trash: bool,
    /// Rename/move is one server operation that never replaces an entry.
    pub atomic_relocate: bool,
}

/// Common provider contract. Every call is one bounded request sequence; the
/// caller owns queueing, journaling and retries of idempotent reads.
pub trait CloudProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> CloudCapabilities;
    /// Stable account identity (not an email); binds Core storage grants.
    fn account_id(&self) -> Result<String>;
    /// Earlier `account_id` formats for the same connection (identity upgrades).
    fn legacy_account_ids(&self) -> Vec<String> { Vec::new() }
    /// Direct child by exact name. More than one match is Unsupported (ambiguous).
    fn lookup(&self, parent: &CloudId, name: &str) -> Result<Option<CloudEntry>>;
    fn quota(&self) -> Result<Quota>;
    fn root(&self) -> Result<CloudEntry>;
    fn metadata(&self, id: &CloudId) -> Result<CloudEntry>;
    /// One page of direct children; `limit` caps the page (1..=1000).
    fn list(&self, folder: &CloudId, page: Option<&str>, limit: usize) -> Result<CloudPage>;
    /// All direct children in one traversal (snapshot), at most `max`.
    /// Default: server pagination with a request cap and repeated-token
    /// detection. Providers without real pagination override it with one
    /// whole-folder request instead of offset slices of repeated listings.
    fn list_all(&self, folder: &CloudId, max: usize) -> Result<Vec<CloudEntry>> {
        let mut out = Vec::new(); let mut page: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..(max / 100 + 10) {
            let chunk = self.list(folder, page.as_deref(), 1000)?;
            out.extend(chunk.entries);
            if out.len() > max { return Err(CoreError::new(ErrorCode::LimitExceeded, "cloud folder exceeds listing limit")); }
            match chunk.next {
                None => return Ok(out),
                Some(token) => { if !seen.insert(token.clone()) { return Err(protocol()); } page = Some(token); }
            }
        }
        Err(CoreError::new(ErrorCode::LimitExceeded, "cloud folder listing needs too many requests"))
    }
    fn create_folder(&self, parent: &CloudId, name: &str) -> Result<CloudEntry>;
    /// Stream bytes from `offset`. Native documents are Unsupported.
    fn open_download(&self, id: &CloudId, offset: u64) -> Result<Box<dyn Read + Send>>;
    /// Upload exactly `bytes` from `source` as a NEW file. AlreadyExists if
    /// the name is taken; NeedsAttention if the outcome is unknown.
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<CloudEntry>;
    /// Move and/or rename; fails if `expected` is stale or the destination name is
    /// taken. Never replaces. May change the id when not `atomic_relocate`.
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> Result<CloudEntry>;
    fn rename(&self, expected: &CloudEntry, name: &str) -> Result<CloudEntry> { self.relocate(expected, None, Some(name)) }
    fn move_to(&self, expected: &CloudEntry, new_parent: &CloudId) -> Result<CloudEntry> { self.relocate(expected, Some(new_parent), None) }
    /// Move to the provider's trash. Folders must be empty.
    fn trash(&self, expected: &CloudEntry) -> Result<()>;
}

/// Portable single-component name (same rules as local ResourcePath). The
/// reserved ".paranoic-" prefix is allowed for adapter staging objects, which
/// CloudStorage hides from Core listings.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.contains('/') || name.is_empty() { return Err(CoreError::new(ErrorCode::InvalidPath, "cloud name must be one component")); }
    // Byte-wise prefix check: the reserved prefix is ASCII, so `get(10..)` is a
    // char boundary whenever it matches; never slice user text at a raw index.
    let check = match name.get(10..) {
        Some(rest) if is_internal(name) && !rest.is_empty() => format!("x{rest}"),
        _ => name.to_owned(),
    };
    paranoic_core::ResourcePath::parse(check).map(|_| ())
}
pub(crate) fn is_internal(name: &str) -> bool { name.len() >= 10 && name.as_bytes()[..10].eq_ignore_ascii_case(b".paranoic-") }
pub(crate) fn stale() -> CoreError { CoreError::new(ErrorCode::SourceChanged, "cloud object changed since it was listed") }
pub(crate) fn exists() -> CoreError { CoreError::new(ErrorCode::AlreadyExists, "a cloud entry with this name already exists") }
pub(crate) fn uncertain() -> CoreError { CoreError::new(ErrorCode::NeedsAttention, "cloud write outcome unknown; inspect before retrying") }
pub(crate) fn protocol() -> CoreError { CoreError::new(ErrorCode::Io, "unexpected cloud provider response") }
pub(crate) fn not_empty() -> CoreError { CoreError::new(ErrorCode::Unsupported, "only empty cloud folders can be trashed") }


fn decimal(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) { return None; }
    value.parse().ok()
}
fn zone_offset(value: &str, colon: bool) -> Option<i64> {
    let b = value.as_bytes();
    if !value.is_ascii() || b.len() != (if colon { 6 } else { 5 })
        || !matches!(b[0], b'+' | b'-') || (colon && b[3] != b':') { return None; }
    let hours = decimal(value.get(1..3)?)?;
    let minutes = decimal(value.get(if colon { 4..6 } else { 3..5 })?)?;
    if hours > 23 || minutes > 59 { return None; }
    Some((if b[0] == b'-' { -1 } else { 1 }) * (hours * 3600 + minutes * 60))
}
#[allow(clippy::too_many_arguments)]
fn calendar_ms(y: i64, m: i64, d: i64, h: i64, minute: i64, s: i64, ms: i64, offset: i64) -> Option<u64> {
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || !(0..=23).contains(&h)
        || !(0..=59).contains(&minute) || !(0..=59).contains(&s) || !(0..=999).contains(&ms) { return None; }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max_day = match m { 2 => if leap { 29 } else { 28 }, 4 | 6 | 9 | 11 => 30, _ => 31 };
    if !(1..=max_day).contains(&d) { return None; }
    // days_from_civil receives only the bounded, validated calendar above.
    let seconds = days_from_civil(y, m, d).checked_mul(86_400)?
        .checked_add(h.checked_mul(3600)?)?.checked_add(minute.checked_mul(60)?)?
        .checked_add(s)?.checked_sub(offset)?;
    u64::try_from(seconds.checked_mul(1000)?.checked_add(ms)?).ok()
}
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400); let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 2822 / HTTP date ("Sat, 24 Jul 2021 12:56:15 +0000" or "... GMT") → Unix ms.
pub(crate) fn http_date_ms(value: &str) -> Option<u64> {
    if !value.is_ascii() || value.len() > 64 { return None; }
    let parts: Vec<&str> = value.split_whitespace().collect();
    let [weekday, day, month, year, time, zone] = parts.as_slice() else { return None };
    if !["Mon,", "Tue,", "Wed,", "Thu,", "Fri,", "Sat,", "Sun,"].contains(weekday)
        || !(1..=2).contains(&day.len()) || year.len() != 4 || time.len() != 8 { return None; }
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| m == month)? as i64 + 1;
    if time.as_bytes()[2] != b':' || time.as_bytes()[5] != b':' { return None; }
    let (h, mi, s) = (decimal(&time[0..2])?, decimal(&time[3..5])?, decimal(&time[6..8])?);
    let offset = match *zone { "GMT" | "UTC" | "Z" => 0, z => zone_offset(z, false)? };
    calendar_ms(decimal(year)?, month, decimal(day)?, h, mi, s, 0, offset)
}

#[cfg(test)]
mod date_tests {
    use super::*;
    #[test]
    fn valid_dates_offsets_and_calendar_boundaries() {
        assert_eq!(http_date_ms("Thu, 01 Jan 1970 01:30:00 +0130"), Some(0));
        assert_eq!(http_date_ms("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    }
    #[test]
    fn untrusted_dates_are_rejected_without_slicing_or_arithmetic_panics() {
        for value in ["Thu, 01 Jan 9223372036854775807 00:00:00 GMT",
            "Thu, 9223372036854775807 Jan 2026 00:00:00 GMT",
            "Thu, 01 Jan 2026 9223372036854775807:00:00 GMT",
            "Thu, 01 Jan 2026 00:00:00 x0000", "Thu, 01 Jan 2026 00:00:00 +0000junk",
            "Thu, 31 Apr 2026 00:00:00 GMT", "Thu, 01 Jan 2026 00:00:00 +2400"] {
            assert_eq!(http_date_ms(value), None, "{value}");
        }
    }
}

