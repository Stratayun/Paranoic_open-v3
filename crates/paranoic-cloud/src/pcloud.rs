//! pCloud HTTP JSON API (regional host from login: api / eapi). File and
//! folder ids are separate namespaces, encoded as "f<id>" / "d<id>".
//! Uploads use `renameifexists=1`: if pCloud had to rename our upload, the
//! fresh object is deleted and AlreadyExists is returned (never overwrite).
//! File rename/move = `copyfile noover=1` (server refuses an existing name),
//! size/hash check of the copy, then the original goes to trash; not atomic
//! (`atomic_relocate=false`), the id changes. renamefile is not used because
//! its overwrite behaviour is undocumented here. Folder rename/move uses
//! renamefolder after a pre-check (residual race; verify live). Tokens do not expire.
use std::{io::Read, sync::{mpsc, Arc}};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CloudCapabilities, CloudEntry, CloudId, CloudKind, CloudPage, CloudProvider, CloudSession, CoreError, ErrorCode,
    ProviderKind, Quota, Result};

const CHUNK: usize = 1 << 20;

pub struct PCloud { session: Arc<CloudSession> }

#[derive(Deserialize, Clone)]
struct Meta { name: String, #[serde(default)] isfolder: bool, folderid: Option<u64>, fileid: Option<u64>, parentfolderid: Option<u64>,
    size: Option<u64>, modified: Option<String>, hash: Option<u64>, contents: Option<Vec<Meta>> }

impl Meta {
    fn id(&self) -> Result<CloudId> {
        match (self.isfolder, self.folderid, self.fileid) {
            (true, Some(id), _) => CloudId::parse(format!("d{id}")),
            (false, _, Some(id)) => CloudId::parse(format!("f{id}")),
            _ => Err(crate::protocol()),
        }
    }
    fn revision(&self) -> Option<String> {
        if self.isfolder { self.modified.clone() } else { self.hash.map(|h| format!("{h}:{}", self.modified.as_deref().unwrap_or(""))) }
    }
    fn entry(&self) -> Result<CloudEntry> {
        Ok(CloudEntry { id: self.id()?, name: self.name.clone(), kind: if self.isfolder { CloudKind::Folder } else { CloudKind::File },
            bytes: if self.isfolder { None } else { Some(self.size.unwrap_or(0)) },
            modified_unix_ms: self.modified.as_deref().and_then(rfc2822_ms), revision: self.revision(), sha256: None, quick_xor: None,
            parents: self.parentfolderid.map(|p| CloudId::parse(format!("d{p}"))).transpose()?.into_iter().collect() })
    }
}

enum Target { File(u64), Folder(u64) }
fn target(id: &CloudId) -> Result<Target> {
    let s = id.as_str();
    let number = |v: &str| v.parse::<u64>().map_err(|_| CoreError::new(ErrorCode::InvalidInput, "invalid pCloud id"));
    match s.split_at_checked(1) { Some(("f", n)) => Ok(Target::File(number(n)?)), Some(("d", n)) => Ok(Target::Folder(number(n)?)),
        _ => Err(CoreError::new(ErrorCode::InvalidInput, "invalid pCloud id")) }
}
fn folder(id: &CloudId) -> Result<u64> { match target(id)? { Target::Folder(n) => Ok(n), Target::File(_) => Err(CoreError::new(ErrorCode::InvalidInput, "pCloud id is not a folder")) } }

fn check(value: serde_json::Value, idempotent: bool) -> Result<serde_json::Value> {
    let code = value.get("result").and_then(|r| r.as_i64()).ok_or_else(crate::protocol)?;
    Err(match code {
        0 => return Ok(value),
        1000 | 2000 | 2094 | 2095 => CoreError::new(ErrorCode::PermissionDenied, "pCloud session expired or revoked"),
        2003 => CoreError::new(ErrorCode::PermissionDenied, "pCloud denied access"),
        2002 | 2005 | 2009 | 2010 => CoreError::new(ErrorCode::NotFound, "pCloud object not found"),
        2004 => crate::exists(),
        2006 => crate::not_empty(),
        2008 => CoreError::new(ErrorCode::LimitExceeded, "pCloud storage quota exceeded"),
        4000 => CoreError::new(ErrorCode::Busy, "pCloud rate limit; retry later"),
        5000 | 5001 if !idempotent => crate::uncertain(),
        5000 | 5001 => CoreError::new(ErrorCode::Busy, "pCloud temporarily unavailable"),
        _ => CoreError::new(ErrorCode::Io, "pCloud rejected the request"),
    })
}
fn meta(value: &serde_json::Value) -> Result<Meta> {
    serde_json::from_value(value.get("metadata").cloned().ok_or_else(crate::protocol)?).map_err(|_| crate::protocol())
}

impl PCloud {
    pub fn new(session: Arc<CloudSession>) -> Result<Self> {
        if session.provider() != ProviderKind::PCloud { return Err(CoreError::new(ErrorCode::InvalidInput, "session is not for pCloud")); }
        session.api_host()?;
        Ok(Self { session })
    }
    fn api(&self, method: &str, query: &[(&str, String)], idempotent: bool) -> Result<serde_json::Value> {
        let host = self.session.api_host()?;
        let http = self.session.http();
        let value: serde_json::Value = self.session.call(|token| http.json(http.client().get(format!("https://{host}/{method}"))
            .query(query).bearer_auth(token), idempotent))?;
        check(value, idempotent)
    }
    fn listing(&self, folder_id: u64, files: bool) -> Result<Meta> {
        let mut query = vec![("folderid", folder_id.to_string())];
        if !files { query.push(("nofiles", "1".into())); }
        meta(&self.api("listfolder", &query, true)?)
    }
    fn stat(&self, id: &CloudId) -> Result<Meta> {
        match target(id)? {
            Target::File(n) => meta(&self.api("stat", &[("fileid", n.to_string())], true)?),
            Target::Folder(n) => { let mut m = self.listing(n, false)?; m.contents = None; Ok(m) }
        }
    }
    fn name_taken(&self, parent: u64, name: &str) -> Result<bool> {
        let lower = name.to_lowercase();
        Ok(self.listing(parent, true)?.contents.unwrap_or_default().iter().any(|m| m.name.to_lowercase() == lower))
    }
    fn current(&self, expected: &CloudEntry) -> Result<Meta> {
        let current = self.stat(&expected.id)?;
        if current.revision() != expected.revision { return Err(crate::stale()); }
        Ok(current)
    }
    /// Strongest hash pCloud reports for a file (sha256 in EU, else sha1).
    fn fingerprint(&self, file: u64) -> Result<String> {
        let value = self.api("checksumfile", &[("fileid", file.to_string())], true)?;
        ["sha256", "sha1"].iter().find_map(|k| value.get(*k).and_then(|v| v.as_str()).map(|v| format!("{k}:{}", v.to_ascii_lowercase())))
            .ok_or_else(crate::protocol)
    }
    fn checksum(&self, file: u64) -> Result<Option<[u8; 32]>> {
        let value = self.api("checksumfile", &[("fileid", file.to_string())], true)?;
        Ok(value.get("sha256").and_then(|v| v.as_str()).and_then(|v| crate::hex32(&v.to_ascii_lowercase())))
    }
}

impl CloudProvider for PCloud {
    fn kind(&self) -> ProviderKind { ProviderKind::PCloud }
    fn capabilities(&self) -> CloudCapabilities {
        CloudCapabilities { resumable_upload: false, ranged_download: true, conditional_mutation: false, atomic_create_new: false,
            duplicate_names: false, server_sha256: true, trash: true, atomic_relocate: false }
    }
    fn quota(&self) -> Result<Quota> {
        let info = self.api("userinfo", &[], true)?;
        Ok(Quota { used: info.get("usedquota").and_then(|v| v.as_u64()), total: info.get("quota").and_then(|v| v.as_u64()) })
    }
    fn account_id(&self) -> Result<String> {
        let info = self.api("userinfo", &[], true)?;
        info.get("userid").and_then(|v| v.as_u64()).map(|v| v.to_string()).ok_or_else(crate::protocol)
    }
    fn lookup(&self, parent: &CloudId, name: &str) -> Result<Option<CloudEntry>> {
        crate::validate_name(name)?;
        let lower = name.to_lowercase();
        let contents = self.listing(folder(parent)?, true)?.contents.unwrap_or_default();
        let mut matches = contents.iter().filter(|m| m.name.to_lowercase() == lower);
        match (matches.next(), matches.next()) {
            (None, _) => Ok(None),
            (Some(m), None) => m.entry().map(Some),
            _ => Err(CoreError::new(ErrorCode::Unsupported, "several pCloud entries share this name")),
        }
    }
    fn root(&self) -> Result<CloudEntry> { let mut m = self.listing(0, false)?; m.contents = None; m.entry() }
    fn metadata(&self, id: &CloudId) -> Result<CloudEntry> { self.stat(id)?.entry() }
    /// One whole-folder request: a consistent snapshot.
    fn list_all(&self, folder_id: &CloudId, max: usize) -> Result<Vec<CloudEntry>> {
        let contents = self.listing(folder(folder_id)?, true)?.contents.unwrap_or_default();
        if contents.len() > max { return Err(CoreError::new(ErrorCode::LimitExceeded, "pCloud folder exceeds listing limit")); }
        contents.iter().map(Meta::entry).collect()
    }
    /// pCloud returns whole folders; pages are slices ("offset:N") of a fresh
    /// listing — use `list_all` for a consistent snapshot.
    fn list(&self, folder_id: &CloudId, page: Option<&str>, limit: usize) -> Result<CloudPage> {
        if !(1..=1000).contains(&limit) { return Err(CoreError::new(ErrorCode::InvalidInput, "page limit out of range")); }
        let offset = match page { None => 0, Some(p) => p.strip_prefix("offset:").and_then(|n| n.parse::<usize>().ok()).ok_or_else(crate::protocol)? };
        let contents = self.listing(folder(folder_id)?, true)?.contents.unwrap_or_default();
        if contents.len() > 1_000_000 { return Err(CoreError::new(ErrorCode::LimitExceeded, "pCloud folder too large")); }
        let end = contents.len().min(offset.saturating_add(limit));
        let entries = contents.get(offset..end).unwrap_or(&[]).iter().map(Meta::entry).collect::<Result<_>>()?;
        Ok(CloudPage { entries, next: (end < contents.len()).then(|| format!("offset:{end}")) })
    }
    fn create_folder(&self, parent: &CloudId, name: &str) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        meta(&self.api("createfolder", &[("folderid", folder(parent)?.to_string()), ("name", name.to_owned())], false)?)?.entry()
    }
    fn open_download(&self, id: &CloudId, offset: u64) -> Result<Box<dyn Read + Send>> {
        let Target::File(file) = target(id)? else { return Err(CoreError::new(ErrorCode::Unsupported, "pCloud folders cannot be downloaded")) };
        let link = self.api("getfilelink", &[("fileid", file.to_string()), ("forcedownload", "1".into())], true)?;
        let host = link.get("hosts").and_then(|h| h.as_array()).and_then(|h| h.first()).and_then(|h| h.as_str()).ok_or_else(crate::protocol)?;
        let path = link.get("path").and_then(|p| p.as_str()).filter(|p| p.starts_with('/')).ok_or_else(crate::protocol)?;
        let url = crate::http::trusted_url(&format!("https://{host}{path}"), &[".pcloud.com"])?;
        let http = self.session.http();
        let mut request = http.client().get(url);
        if offset > 0 { request = request.header(reqwest::header::RANGE, format!("bytes={offset}-")); }
        let response = http.ok(request, true)?;
        if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT { return Err(crate::protocol()); }
        Ok(Box::new(response))
    }
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        let parent_id = folder(parent)?;
        progress(0)?;
        if self.name_taken(parent_id, name)? { return Err(crate::exists()); }
        let host = self.session.api_host()?; let token = self.session.bearer()?;
        let http = self.session.http().clone();
        let url = reqwest::Url::parse_with_params(&format!("https://{host}/uploadfile"), &[("folderid", parent_id.to_string()),
            ("filename", name.to_owned()), ("nopartial", "1".into()), ("renameifexists", "1".into())]).map_err(|_| crate::protocol())?;
        let mut hash = Sha256::new();
        let (sent, response) = std::thread::scope(|scope| {
            let (tx, rx) = mpsc::sync_channel::<std::result::Result<Zeroizing<Vec<u8>>, ()>>(4);
            let body = reqwest::blocking::Body::sized(crate::transfer::ChannelReader { rx, current: Zeroizing::new(Vec::new()), position: 0 }, bytes);
            let request = http.client().put(url).bearer_auth(token.as_str()).body(body);
            let worker = scope.spawn(move || http.send(request, false));
            let mut sent = 0u64;
            let pumped: Result<()> = (|| {
                while sent < bytes {
                    progress(sent)?;
                    let mut chunk = Zeroizing::new(vec![0u8; (bytes - sent).min(CHUNK as u64) as usize]);
                    let n = crate::transfer::fill(source, &mut chunk)?;
                    if n < chunk.len() { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source shrank")); }
                    hash.update(&chunk[..]); sent += n as u64;
                    if tx.send(Ok(chunk)).is_err() { break; } // request ended early
                }
                let mut tail = [0u8; 1];
                if source.read(&mut tail).map_err(CoreError::io)? != 0 { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source grew")); }
                Ok(())
            })();
            if pumped.is_err() { let _ = tx.send(Err(())); }
            drop(tx);
            let response = worker.join().map_err(|_| CoreError::new(ErrorCode::Internal, "upload worker failed"))?;
            pumped?; Ok::<_, CoreError>((sent, response?))
        })?;
        if !response.status().is_success() { return Err(crate::http::status_error(&response, false)); }
        // After a 2xx the upload may exist: an unreadable confirmation is uncertain (audit 9).
        let value = check(crate::http::read_json(response).map_err(|_| crate::uncertain())?, false)?;
        let created: Meta = value.get("metadata").and_then(|m| m.as_array()).and_then(|m| m.first()).cloned()
            .map(serde_json::from_value).transpose().map_err(|_| crate::protocol())?.ok_or_else(crate::uncertain)?;
        let file = created.fileid.ok_or_else(crate::uncertain)?;
        if created.name != name {
            // pCloud renamed our fresh upload because the name appeared concurrently.
            let _ = self.api("deletefile", &[("fileid", file.to_string())], false);
            return Err(crate::exists());
        }
        let digest: [u8; 32] = hash.finalize().into();
        let server = self.checksum(file)?;
        if created.size != Some(bytes) || sent != bytes || server.is_some_and(|s| s != digest) {
            return Err(CoreError::new(ErrorCode::NeedsAttention, "uploaded pCloud file does not match the source"));
        }
        progress(sent)?;
        let mut entry = created.entry()?; entry.sha256 = server; Ok(entry)
    }
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> Result<CloudEntry> {
        if let Some(name) = new_name { crate::validate_name(name)?; }
        let current = self.current(expected)?;
        let destination = match new_parent { Some(p) => folder(p)?, None => current.parentfolderid.ok_or_else(crate::protocol)? };
        let name = new_name.unwrap_or(&current.name).to_owned();
        if self.name_taken(destination, &name)? { return Err(crate::exists()); }
        match target(&expected.id)? {
            Target::Folder(n) => meta(&self.api("renamefolder", &[("folderid", n.to_string()), ("tofolderid", destination.to_string()),
                ("toname", name)], false)?)?.entry(),
            Target::File(n) => {
                let copy = meta(&self.api("copyfile", &[("fileid", n.to_string()), ("tofolderid", destination.to_string()),
                    ("toname", name.clone()), ("noover", "1".into())], false)?)?;
                let id = copy.fileid.ok_or_else(crate::uncertain)?;
                let (original, copied) = (self.fingerprint(n)?, self.fingerprint(id)?);
                if copy.name != name || copy.size != current.size || original != copied {
                    return Err(CoreError::new(ErrorCode::NeedsAttention, "pCloud copy differs; original kept"));
                }
                // The original goes to pCloud trash only after the copy was verified.
                self.api("deletefile", &[("fileid", n.to_string())], false)
                    .map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "pCloud copy created but original not removed"))?;
                let mut entry = copy.entry()?; entry.sha256 = self.checksum(id)?; Ok(entry)
            }
        }
    }
    fn trash(&self, expected: &CloudEntry) -> Result<()> {
        self.current(expected)?;
        match target(&expected.id)? {
            Target::File(n) => self.api("deletefile", &[("fileid", n.to_string())], false)?,
            Target::Folder(n) => self.api("deletefolder", &[("folderid", n.to_string())], false)?, // fails (2006) unless empty
        };
        Ok(())
    }
}

fn rfc2822_ms(value: &str) -> Option<u64> { crate::http_date_ms(value) }
