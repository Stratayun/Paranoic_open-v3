//! WebDAV (RFC 4918) provider: the own Paranoic server, Nextcloud/ownCloud,
//! and other standard servers. Guarantees come from standard headers:
//! * create-new upload: `PUT` + `If-None-Match: *` (412 if the name exists);
//! * folders: `MKCOL` (405 if it exists);
//! * rename/move: `MOVE` + `Overwrite: F` + `If-Match: <etag>` — one request,
//!   never replaces (the own server enforces it with a no-replace rename);
//! * deletion: `DELETE` with `If-Match`; on Nextcloud and the own server it
//!   lands in the trash (`server_trash`), elsewhere it may be permanent.
//!
//! Ids are the percent-encoded path below the base URL ("/" = root); they
//! change on rename/move. Credentials: Basic over HTTPS (HTTP only to loopback).
use std::{io::Read, sync::Arc};
use quick_xml::{events::Event, Reader};
use reqwest::{Method, Url};
use sha1::Sha1;
use sha2::Digest;
use zeroize::Zeroizing;
use crate::{CloudCapabilities, CloudEntry, CloudId, CloudKind, CloudPage, CloudProvider, CoreError, ErrorCode, Http, ProviderKind, Quota, Result};

const PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:getetag/><oc:checksums/></d:prop></d:propfind>"#;
const QUOTA: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:quota-used-bytes/><d:quota-available-bytes/></d:prop></d:propfind>"#;
const MAX_XML: u64 = 64 << 20;

pub struct WebDavCredentials { pub username: String, pub password: Zeroizing<String> }

pub struct WebDav { http: Http, base: Url, credentials: Arc<WebDavCredentials>, server_trash: bool }

#[derive(Default, Debug)]
struct Props { href: String, collection: bool, length: Option<u64>, modified: Option<String>, etag: Option<String>, sha1: Option<String>, used: Option<u64>, available: Option<u64> }

impl WebDav {
    /// `base` must be an https:// URL ending with "/" (http:// only for 127.0.0.1/localhost,
    /// which also needs an `Http::new_loopback_http()` client).
    /// `server_trash`: the server moves deleted items to a trash (own server, Nextcloud).
    pub fn new(http: Http, base: &str, credentials: WebDavCredentials, server_trash: bool) -> Result<Self> {
        let base = Url::parse(base).map_err(|_| invalid())?;
        let loopback = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !(base.scheme() == "https" || (base.scheme() == "http" && loopback)) || !base.path().ends_with('/')
            || base.query().is_some() || !base.username().is_empty() || credentials.username.contains(':') {
            return Err(invalid());
        }
        Ok(Self { http, base, credentials: Arc::new(credentials), server_trash })
    }
    /// Ask the server whether DELETE is recoverable (У-35). Only a specific
    /// positive answer counts, from an authenticated request to the same origin:
    /// * `paranoic-server`: header `X-Paranoic-Trash: 1` on OPTIONS of `base`;
    /// * Nextcloud/ownCloud: `base` is below `/remote.php/dav/files/<user>/` and
    ///   the user's trashbin collection `/remote.php/dav/trashbin/<user>/trash/`
    ///   answers PROPFIND with 207 (trashbin app enabled).
    ///
    /// Any other server, error or answer is `false`: deletion stays refused.
    /// The server is trusted for this as much as it is trusted with the files.
    pub fn probe_trash(&self) -> bool {
        let Ok(options) = Method::from_bytes(b"OPTIONS") else { return false };
        if let Ok(response) = self.http.send(self.request(options, self.base.clone()), true) {
            if response.status().is_success()
                && response.headers().get("x-paranoic-trash").is_some_and(|v| v.as_bytes() == b"1") { return true; }
        }
        let Some(rest) = self.base.path().strip_prefix("/remote.php/dav/files/") else { return false };
        let Some(user) = rest.split('/').next().filter(|u| !u.is_empty()) else { return false };
        let mut url = self.base.clone();
        url.set_path(&format!("/remote.php/dav/trashbin/{user}/trash/"));
        if url.origin() != self.base.origin() { return false; }
        let Ok(propfind) = Method::from_bytes(b"PROPFIND") else { return false };
        let body = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#;
        self.http.send(self.request(propfind, url).header("Depth", "0")
            .header(reqwest::header::CONTENT_TYPE, "application/xml; charset=utf-8").body(body), true)
            .is_ok_and(|response| response.status().as_u16() == 207)
    }
    /// Use the result of [`Self::probe_trash`] (or an explicit confirmation).
    pub fn with_server_trash(mut self, confirmed: bool) -> Self { self.server_trash = confirmed; self }
    /// Build the URL only from validated, percent-encoded path segments below
    /// `base`; the result must keep base's origin and path prefix, otherwise the
    /// request (which carries credentials) is refused.
    fn url(&self, id: &CloudId) -> Result<Url> {
        let path = id.as_str();
        let rest = path.strip_prefix('/').ok_or_else(invalid)?;
        let folder = rest.ends_with('/');
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| invalid())?;
            segments.pop_if_empty();
            for part in rest.split('/').filter(|p| !p.is_empty()) {
                let decoded = percent_decode(part).ok_or_else(invalid)?;
                if decoded == "." || decoded == ".." || decoded.contains(['/', '\\']) || decoded.chars().any(char::is_control) { return Err(invalid()); }
                segments.push(&decoded);
            }
            if folder { segments.push(""); }
        }
        if url.origin() != self.base.origin() || !url.path().starts_with(self.base.path()) { return Err(invalid()); }
        Ok(url)
    }
    fn child_id(parent: &CloudId, name: &str, folder: bool) -> Result<CloudId> {
        crate::validate_name(name)?;
        let encoded: String = name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        let parent = parent.as_str().trim_end_matches('/');
        CloudId::parse(format!("{parent}/{encoded}{}", if folder { "/" } else { "" }))
    }
    fn request(&self, method: Method, url: Url) -> reqwest::blocking::RequestBuilder {
        // Redirects are not followed (Http policy), so credentials stay on this origin.
        self.http.client().request(method, url).basic_auth(&self.credentials.username, Some(self.credentials.password.as_str()))
    }
    fn propfind(&self, id: &CloudId, depth: &str, body: &'static str) -> Result<Vec<Props>> {
        let response = self.http.send(self.request(Method::from_bytes(b"PROPFIND").map_err(|_| invalid())?, self.url(id)?)
            .header("Depth", depth).header(reqwest::header::CONTENT_TYPE, "application/xml; charset=utf-8").body(body), true)?;
        if response.status().as_u16() != 207 { return Err(crate::http::status_error(&response, true)); }
        let mut xml = Vec::new();
        response.take(MAX_XML + 1).read_to_end(&mut xml).map_err(CoreError::io)?;
        if xml.len() as u64 > MAX_XML { return Err(CoreError::new(ErrorCode::LimitExceeded, "WebDAV listing too large")); }
        parse_multistatus(&xml)
    }
    /// Convert an href from a multistatus into our id (must be below base).
    fn id_of(&self, href: &str) -> Result<CloudId> {
        let path = if href.starts_with('/') { href.to_owned() } else {
            let url = Url::parse(href).map_err(|_| crate::protocol())?;
            if url.origin() != self.base.origin() { return Err(crate::protocol()); }
            url.path().to_owned()
        };
        let rest = path.strip_prefix(self.base.path()).ok_or_else(crate::protocol)?;
        // Normalize percent-encoding to our canonical form (decode, re-encode).
        let folder = rest.ends_with('/') || rest.is_empty();
        let mut id = CloudId::parse("/")?;
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            let decoded = percent_decode(part).ok_or_else(crate::protocol)?;
            id = Self::child_id(&id, &decoded, false)?;
        }
        if folder && id.as_str() != "/" { id = CloudId::parse(format!("{}/", id.as_str()))?; }
        Ok(id)
    }
    fn entry(&self, props: Props) -> Result<CloudEntry> {
        let id = self.id_of(&props.href)?;
        let trimmed = id.as_str().trim_end_matches('/');
        let name = percent_decode(trimmed.rsplit('/').next().unwrap_or("")).ok_or_else(crate::protocol)?;
        let parent = trimmed.rsplit_once('/').map(|(p, _)| format!("{p}/"));
        let kind = if props.collection { CloudKind::Folder } else { CloudKind::File };
        let id = if kind == CloudKind::Folder && !id.as_str().ends_with('/') { CloudId::parse(format!("{}/", id.as_str()))? } else { id };
        Ok(CloudEntry { id, name, kind, bytes: (kind == CloudKind::File).then(|| props.length.unwrap_or(0)),
            modified_unix_ms: props.modified.as_deref().and_then(crate::http_date_ms), revision: props.etag,
            sha256: None,
            parents: parent.filter(|p| !p.is_empty()).map(CloudId::parse).transpose()?.into_iter().collect() })
    }
    fn stat(&self, id: &CloudId) -> Result<(CloudEntry, Option<String>)> {
        let mut found = self.propfind(id, "0", PROPFIND)?;
        if found.len() != 1 { return Err(crate::protocol()); }
        let props = found.remove(0);
        let sha1 = props.sha1.clone();
        Ok((self.entry(props)?, sha1))
    }
    /// One PROPFIND Depth:1 of a folder (whole listing, sorted by name).
    fn children(&self, folder: &CloudId) -> Result<Vec<CloudEntry>> {
        // Compare canonical ids, not raw hrefs: servers differ in percent-encoding
        // (e.g. '=' as %3D), and the folder itself must never appear as its child.
        let own = folder.as_str().trim_end_matches('/').to_owned();
        let mut children = Vec::new();
        for props in self.propfind(folder, "1", PROPFIND)? {
            if self.id_of(&props.href)?.as_str().trim_end_matches('/') == own { continue; }
            children.push(self.entry(props)?);
        }
        children.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(children)
    }
    fn destination(&self, parent: &CloudId, name: &str, folder: bool) -> Result<Url> { self.url(&Self::child_id(parent, name, folder)?) }
}

impl CloudProvider for WebDav {
    fn kind(&self) -> ProviderKind { ProviderKind::WebDav }
    fn capabilities(&self) -> CloudCapabilities {
        // conditional_mutation relies on ETags; entries without one are refused in relocate/trash.
        CloudCapabilities { resumable_upload: false, ranged_download: true, conditional_mutation: true, atomic_create_new: true,
            duplicate_names: false, server_sha256: false, trash: self.server_trash, atomic_relocate: true }
    }
    fn account_id(&self) -> Result<String> {
        // Versioned, unambiguous identity: URL canonicalization includes the
        // scheme and effective port (and normalizes an explicit default port).
        // Length prefixes also separate usernames containing '@' or '/'.
        let origin = self.base.origin().ascii_serialization();
        let path = self.base.path();
        let user = &self.credentials.username;
        Ok(format!("webdav-v2:{}:{origin}{}:{path}{}:{user}", origin.len(), path.len(), user.len()))
    }
    /// v1 identity (before У-38) of the same saved URL and user. Lets a grant
    /// saved before the upgrade keep its storage ID and its jobs: the journal
    /// accepts it only if its stored binding is exactly this value.
    fn legacy_account_ids(&self) -> Vec<String> {
        vec![format!("{}@{}{}", self.credentials.username, self.base.host_str().unwrap_or(""), self.base.path())]
    }
    fn lookup(&self, parent: &CloudId, name: &str) -> Result<Option<CloudEntry>> {
        match self.stat(&Self::child_id(parent, name, false)?) {
            Ok((entry, _)) if entry.name == name => Ok(Some(entry)),
            Ok(_) => Err(CoreError::new(ErrorCode::Unsupported, "WebDAV server returned another name (case folding)")),
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn quota(&self) -> Result<Quota> {
        let props = self.propfind(&CloudId::parse("/")?, "0", QUOTA)?;
        let p = props.first().ok_or_else(crate::protocol)?;
        Ok(Quota { used: p.used, total: match (p.used, p.available) { (Some(u), Some(a)) => u.checked_add(a), _ => None } })
    }
    fn root(&self) -> Result<CloudEntry> { Ok(self.stat(&CloudId::parse("/")?)?.0) }
    fn metadata(&self, id: &CloudId) -> Result<CloudEntry> { Ok(self.stat(id)?.0) }
    fn list_all(&self, folder: &CloudId, max: usize) -> Result<Vec<CloudEntry>> {
        let children = self.children(folder)?;
        if children.len() > max { return Err(CoreError::new(ErrorCode::LimitExceeded, "WebDAV folder exceeds listing limit")); }
        Ok(children)
    }
    /// Pages are slices of one listing; use `list_all` for a consistent snapshot.
    fn list(&self, folder: &CloudId, page: Option<&str>, limit: usize) -> Result<CloudPage> {
        if !(1..=1000).contains(&limit) { return Err(invalid()); }
        let offset = match page { None => 0, Some(p) => p.strip_prefix("offset:").and_then(|n| n.parse::<usize>().ok()).ok_or_else(crate::protocol)? };
        let mut children = self.children(folder)?;
        let end = children.len().min(offset.saturating_add(limit));
        let next = (end < children.len()).then(|| format!("offset:{end}"));
        Ok(CloudPage { entries: children.drain(offset.min(end)..end).collect(), next })
    }
    fn create_folder(&self, parent: &CloudId, name: &str) -> Result<CloudEntry> {
        let url = self.destination(parent, name, true)?;
        let response = self.http.send(self.request(Method::from_bytes(b"MKCOL").map_err(|_| invalid())?, url), false)?;
        match response.status().as_u16() {
            201 => self.metadata(&Self::child_id(parent, name, true)?),
            405 => Err(crate::exists()),
            _ => Err(crate::http::status_error(&response, false)),
        }
    }
    fn open_download(&self, id: &CloudId, offset: u64) -> Result<Box<dyn Read + Send>> {
        let mut request = self.request(Method::GET, self.url(id)?);
        if offset > 0 { request = request.header(reqwest::header::RANGE, format!("bytes={offset}-")); }
        let response = self.http.ok(request, true)?;
        if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT { return Err(crate::protocol()); }
        Ok(Box::new(response))
    }
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<CloudEntry> {
        let id = Self::child_id(parent, name, false)?;
        let url = self.url(&id)?;
        let mut sha1 = Sha1::new();
        let request = self.request(Method::PUT, url).header(reqwest::header::IF_NONE_MATCH, "*")
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
        let http = self.http.clone();
        let response = crate::transfer::streamed(bytes, source, progress, &mut |chunk| sha1.update(chunk),
            move |body| http.send(request.body(body), false))?;
        match response.status().as_u16() {
            201 | 204 | 200 => {}
            412 => return Err(crate::exists()),
            _ => return Err(crate::http::status_error(&response, false)),
        }
        let (mut entry, server_sha1) = self.stat(&id)?;
        let local_sha1: String = sha1.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if entry.bytes != Some(bytes) || server_sha1.is_some_and(|s| s != local_sha1) {
            return Err(CoreError::new(ErrorCode::NeedsAttention, "uploaded WebDAV file does not match the source"));
        }
        // No server SHA-256: verify_remote proves content by a download readback.
        entry.sha256 = None;
        Ok(entry)
    }
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> Result<CloudEntry> {
        // Files must carry an ETag so the move is conditional on the exact version.
        if expected.kind != CloudKind::Folder && expected.revision.is_none() {
            return Err(CoreError::new(ErrorCode::Unsupported, "WebDAV entry has no ETag; conditional move impossible"));
        }
        let folder = expected.kind == CloudKind::Folder;
        let parent = match new_parent { Some(p) => p.clone(), None => expected.parents.first().cloned().ok_or_else(crate::protocol)? };
        let name = new_name.unwrap_or(&expected.name);
        let target = Self::child_id(&parent, name, folder)?;
        let mut request = self.request(Method::from_bytes(b"MOVE").map_err(|_| invalid())?, self.url(&expected.id)?)
            .header("Destination", self.url(&target)?.as_str()).header("Overwrite", "F");
        if let Some(etag) = &expected.revision { request = request.header(reqwest::header::IF_MATCH, quoted(etag)); }
        let response = self.http.send(request, false)?;
        match response.status().as_u16() {
            201 | 204 => self.metadata(&target),
            412 => {
                // Either the destination exists or the source changed: tell them apart.
                match self.stat(&expected.id) { Ok((current, _)) if current.revision == expected.revision => Err(crate::exists()), _ => Err(crate::stale()) }
            }
            _ => Err(crate::http::status_error(&response, false)),
        }
    }
    fn trash(&self, expected: &CloudEntry) -> Result<()> {
        // DELETE is permanent unless the server keeps a trash (own server,
        // Nextcloud). Without that confirmation, "trash" is refused.
        if !self.server_trash { return Err(CoreError::new(ErrorCode::Unsupported, "this WebDAV server has no confirmed trash; deletion refused")); }
        if expected.revision.is_none() { return Err(CoreError::new(ErrorCode::Unsupported, "WebDAV entry has no ETag; conditional deletion impossible")); }
        let (current, _) = self.stat(&expected.id)?;
        if current.revision != expected.revision { return Err(crate::stale()); }
        if current.kind == CloudKind::Folder && !self.list(&expected.id, None, 1)?.entries.is_empty() { return Err(crate::not_empty()); }
        let mut request = self.request(Method::DELETE, self.url(&expected.id)?);
        if let Some(etag) = &expected.revision { request = request.header(reqwest::header::IF_MATCH, quoted(etag)); }
        let response = self.http.send(request, false)?;
        match response.status().as_u16() { 200 | 204 => Ok(()), 412 => Err(crate::stale()), _ => Err(crate::http::status_error(&response, false)) }
    }
}

/// If-Match needs an entity-tag in quotes (RFC 9110). Some servers (dav-server)
/// report getetag without them; sending it bare makes every condition fail.
fn quoted(etag: &str) -> String {
    if etag.starts_with('"') || etag.starts_with("W/\"") { etag.to_owned() } else { format!("\"{etag}\"") }
}
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid WebDAV request") }
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes(); let mut out = Vec::with_capacity(bytes.len()); let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' { out.push(u8::from_str_radix(value.get(i + 1..i + 3)?, 16).ok()?); i += 3; } else { out.push(bytes[i]); i += 1; }
    }
    String::from_utf8(out).ok()
}
/// Parse a 207 multistatus; only properties from a 200 propstat are used.
fn parse_multistatus(xml: &[u8]) -> Result<Vec<Props>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new(); let mut current: Option<Props> = None; let mut pending = Props::default();
    let mut path: Vec<Vec<u8>> = Vec::new(); let mut status_ok = false; let mut text = String::new();
    loop {
        match reader.read_event().map_err(|_| crate::protocol())? {
            Event::Eof => break,
            Event::DocType(_) => return Err(crate::protocol()),
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_vec();
                if path.len() > 32 { return Err(crate::protocol()); }
                // <collection/> and <collection></collection> are equivalent (dav-server sends the latter).
                match name.as_slice() { b"response" => current = Some(Props::default()), b"propstat" => { pending = Props::default(); status_ok = false; }
                    b"collection" => pending.collection = true, _ => {} }
                path.push(name); text.clear();
            }
            Event::Empty(e) => if e.local_name().as_ref() == b"collection" { pending.collection = true; },
            Event::Text(t) => { text.push_str(&t.xml10_content().map_err(|_| crate::protocol())?); if text.len() > 65_536 { return Err(crate::protocol()); } }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(|_| crate::protocol())? { Some(c) => c.to_string(), None => match r.decode().map_err(|_| crate::protocol())?.as_ref() {
                    "amp" => "&".into(), "lt" => "<".into(), "gt" => ">".into(), "quot" => "\"".into(), "apos" => "'".into(), _ => return Err(crate::protocol()) } };
                text.push_str(&resolved);
            }
            Event::End(_) => {
                let name = path.pop().unwrap_or_default();
                let value = std::mem::take(&mut text);
                match name.as_slice() {
                    b"href" if path.last().is_some_and(|p| p == b"response") => if let Some(c) = current.as_mut() { c.href = value },
                    b"status" if path.last().is_some_and(|p| p == b"propstat") => status_ok = value.contains(" 200"),
                    b"getcontentlength" => pending.length = value.trim().parse().ok(),
                    b"getlastmodified" => pending.modified = Some(value),
                    b"getetag" => pending.etag = Some(value),
                    b"quota-used-bytes" => pending.used = value.trim().parse().ok(),
                    b"quota-available-bytes" => pending.available = value.trim().parse().ok(),
                    b"checksum" | b"checksums" => if let Some(s) = value.split_whitespace().find_map(|c| c.strip_prefix("SHA1:")) { pending.sha1 = Some(s.to_ascii_lowercase()) },
                    b"propstat" => if status_ok { if let Some(c) = current.as_mut() {
                        let p = std::mem::take(&mut pending);
                        c.collection |= p.collection; c.length = c.length.or(p.length); c.modified = c.modified.take().or(p.modified);
                        c.etag = c.etag.take().or(p.etag); c.sha1 = c.sha1.take().or(p.sha1); c.used = c.used.or(p.used); c.available = c.available.or(p.available);
                    } },
                    b"response" => if let Some(c) = current.take() {
                        if !c.href.is_empty() { out.push(c); }
                        if out.len() > 200_000 { return Err(crate::protocol()); }
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod identity_tests {
    use super::*;
    fn dav(url: &str, user: &str, confirmed_trash: bool) -> WebDav {
        WebDav::new(Http::new_loopback_http().unwrap(), url, WebDavCredentials {
            username: user.to_owned(), password: Zeroizing::new("private-token".into()) }, confirmed_trash).unwrap()
    }
    #[test]
    fn origin_root_and_account_are_separate_identity_fields() {
        let identity = |url, user| dav(url, user, false).account_id().unwrap();
        let a = identity("https://example.com/root/", "me");
        assert_eq!(a, identity("https://EXAMPLE.com:443/root/", "me"));
        assert_ne!(a, identity("https://example.com:8443/root/", "me"));
        assert_ne!(a, identity("https://example.com/other/", "me"));
        assert_ne!(a, identity("https://example.com/root/", "other"));
        assert_ne!(identity("http://localhost/root/", "me"), identity("https://localhost/root/", "me"));
        // These collided in username@host/path formatting.
        assert_ne!(identity("https://b.com/root/", "x@a.com/y"),
            identity("https://a.com/y@b.com/root/", "x"));
        assert!(!a.contains("private-token"));
    }
    #[test]
    fn unconfirmed_trash_is_refused_before_any_network_request() {
        let provider = dav("http://127.0.0.1:1/", "me", false);
        assert!(!provider.capabilities().trash);
        let entry = CloudEntry { id: CloudId::parse("/file").unwrap(), name: "file".into(),
            kind: CloudKind::File, bytes: Some(1), modified_unix_ms: None,
            revision: Some("etag".into()), sha256: None, parents: vec![] };
        assert_eq!(provider.trash(&entry).unwrap_err().code, ErrorCode::Unsupported);
        // Only a caller which actually knows its server's behavior may opt in.
        assert!(dav("http://127.0.0.1:1/", "me", true).capabilities().trash);
    }
}

