//! Google Drive v3 (REST). Drive allows duplicate names, so create-new is a
//! pre-check plus post-verification (residual race documented). Google-native
//! documents (Docs/Sheets/...) are listed as `Native` and not downloadable.
use std::{io::Read, sync::Arc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CloudCapabilities, CloudEntry, CloudId, CloudKind, CloudPage, CloudProvider, CloudSession, CoreError, ErrorCode,
    ProviderKind, Quota, Result};

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FIELDS: &str = "id,name,mimeType,size,modifiedTime,sha256Checksum,version,parents,trashed";
const FOLDER: &str = "application/vnd.google-apps.folder";
const CHUNK: usize = 8 << 20; // multiple of 256 KiB

pub struct GoogleDrive { session: Arc<CloudSession> }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File { id: String, name: String, mime_type: String, size: Option<String>, modified_time: Option<String>,
    sha256_checksum: Option<String>, version: Option<String>, parents: Option<Vec<String>>, trashed: Option<bool> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList { files: Vec<File>, next_page_token: Option<String> }
/// Minimal response for name checks (`fields=files(id)`).
#[derive(Deserialize)]
struct IdList { files: Vec<IdOnly> }
#[derive(Deserialize)]
struct IdOnly { #[allow(dead_code)] id: String }

impl File {
    fn entry(self) -> Result<CloudEntry> {
        let kind = if self.mime_type == FOLDER { CloudKind::Folder }
            else if self.mime_type.starts_with("application/vnd.google-apps.") { CloudKind::Native } else { CloudKind::File };
        Ok(CloudEntry { id: CloudId::parse(self.id)?, name: self.name, kind,
            bytes: if kind == CloudKind::File { Some(self.size.as_deref().unwrap_or("0").parse().map_err(|_| crate::protocol())?) } else { None },
            modified_unix_ms: self.modified_time.as_deref().and_then(crate::rfc3339_ms),
            revision: self.version, sha256: self.sha256_checksum.as_deref().and_then(crate::hex32), quick_xor: None,
            parents: self.parents.unwrap_or_default().into_iter().map(CloudId::parse).collect::<Result<_>>()? })
    }
}

fn drive_id(id: &CloudId) -> Result<&str> {
    let s = id.as_str();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { Ok(s) } else { Err(CoreError::new(ErrorCode::InvalidInput, "invalid Drive id")) }
}
fn quote(value: &str) -> String { value.replace('\\', "\\\\").replace('\'', "\\'") }

impl GoogleDrive {
    pub fn new(session: Arc<CloudSession>) -> Result<Self> {
        if session.provider() != ProviderKind::GoogleDrive { return Err(CoreError::new(ErrorCode::InvalidInput, "session is not for Google Drive")); }
        Ok(Self { session })
    }
    fn get_file(&self, id: &str) -> Result<File> {
        let http = self.session.http();
        self.session.call(|token| http.json(http.client().get(format!("{API}/files/{id}"))
            .query(&[("fields", FIELDS), ("supportsAllDrives", "true")]).bearer_auth(token), true))
    }
    fn child_named(&self, parent: &str, name: &str) -> Result<bool> {
        let http = self.session.http();
        let q = format!("'{parent}' in parents and name = '{}' and trashed = false", quote(name));
        let list: IdList = self.session.call(|token| http.json(http.client().get(format!("{API}/files"))
            .query(&[("q", q.as_str()), ("fields", "files(id)"), ("pageSize", "1"), ("supportsAllDrives", "true"),
                ("includeItemsFromAllDrives", "true")]).bearer_auth(token), true))?;
        Ok(!list.files.is_empty())
    }
    /// Runs after the object exists: any failure here is reported as an
    /// unknown outcome (the created entry stays), never as a clean error.
    fn unique_after(&self, parent: &str, name: &str, created: &CloudEntry) -> Result<CloudEntry> {
        self.unique_after_inner(parent, name, created).map_err(|e| if e.code == ErrorCode::NeedsAttention { e } else {
            CoreError::new(ErrorCode::NeedsAttention, "Drive entry created but post-check failed; inspect before retrying") })
    }
    fn unique_after_inner(&self, parent: &str, name: &str, created: &CloudEntry) -> Result<CloudEntry> {
        // Post-verification: if a concurrent writer created the same name, report it.
        let http = self.session.http();
        let q = format!("'{parent}' in parents and name = '{}' and trashed = false", quote(name));
        let list: IdList = self.session.call(|token| http.json(http.client().get(format!("{API}/files"))
            .query(&[("q", q.as_str()), ("fields", "files(id)"), ("pageSize", "2"), ("supportsAllDrives", "true"),
                ("includeItemsFromAllDrives", "true")]).bearer_auth(token), true))?;
        if list.files.len() > 1 { return Err(CoreError::new(ErrorCode::NeedsAttention, "another Drive entry with the same name appeared concurrently")); }
        Ok(created.clone())
    }
    fn patch(&self, id: &str, query: &[(&str, &str)], body: serde_json::Value) -> Result<CloudEntry> {
        let http = self.session.http();
        let mut params = vec![("fields", FIELDS), ("supportsAllDrives", "true")]; params.extend_from_slice(query);
        let file: File = self.session.call(|token| http.json(http.client().patch(format!("{API}/files/{id}"))
            .query(&params).bearer_auth(token).json(&body), false))?;
        file.entry()
    }
    fn current(&self, expected: &CloudEntry) -> Result<File> {
        let file = self.get_file(drive_id(&expected.id)?)?;
        if file.version != expected.revision || file.trashed == Some(true) { return Err(crate::stale()); }
        Ok(file)
    }
}

impl CloudProvider for GoogleDrive {
    fn kind(&self) -> ProviderKind { ProviderKind::GoogleDrive }
    fn capabilities(&self) -> CloudCapabilities {
        CloudCapabilities { resumable_upload: true, ranged_download: true, conditional_mutation: false, atomic_create_new: false,
            duplicate_names: true, server_sha256: true, trash: true, atomic_relocate: true }
    }
    fn quota(&self) -> Result<Quota> {
        #[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct About { storage_quota: Q }
        #[derive(Deserialize)] struct Q { limit: Option<String>, usage: Option<String> }
        let http = self.session.http();
        let about: About = self.session.call(|token| http.json(http.client().get(format!("{API}/about"))
            .query(&[("fields", "storageQuota(limit,usage)")]).bearer_auth(token), true))?;
        Ok(Quota { used: about.storage_quota.usage.and_then(|v| v.parse().ok()), total: about.storage_quota.limit.and_then(|v| v.parse().ok()) })
    }
    fn account_id(&self) -> Result<String> {
        #[derive(Deserialize)] struct About { user: User }
        #[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct User { permission_id: String }
        let http = self.session.http();
        let about: About = self.session.call(|token| http.json(http.client().get(format!("{API}/about"))
            .query(&[("fields", "user(permissionId)")]).bearer_auth(token), true))?;
        Ok(about.user.permission_id)
    }
    fn lookup(&self, parent: &CloudId, name: &str) -> Result<Option<CloudEntry>> {
        crate::validate_name(name)?;
        let http = self.session.http();
        let q = format!("'{}' in parents and name = '{}' and trashed = false", drive_id(parent)?, quote(name));
        let fields = format!("files({FIELDS})");
        let list: FileList = self.session.call(|token| http.json(http.client().get(format!("{API}/files"))
            .query(&[("q", q.as_str()), ("fields", fields.as_str()), ("pageSize", "2"), ("supportsAllDrives", "true"),
                ("includeItemsFromAllDrives", "true")]).bearer_auth(token), true))?;
        let mut files = list.files.into_iter();
        match (files.next(), files.next()) {
            (None, _) => Ok(None),
            (Some(file), None) => file.entry().map(Some),
            _ => Err(CoreError::new(ErrorCode::Unsupported, "several Drive entries share this name")),
        }
    }
    fn root(&self) -> Result<CloudEntry> { self.get_file("root")?.entry() }
    fn metadata(&self, id: &CloudId) -> Result<CloudEntry> { self.get_file(drive_id(id)?)?.entry() }
    fn list(&self, folder: &CloudId, page: Option<&str>, limit: usize) -> Result<CloudPage> {
        if !(1..=1000).contains(&limit) { return Err(CoreError::new(ErrorCode::InvalidInput, "page limit out of range")); }
        let q = format!("'{}' in parents and trashed = false", drive_id(folder)?);
        let size = limit.to_string(); let fields = format!("nextPageToken,files({FIELDS})");
        let mut params = vec![("q", q.as_str()), ("fields", fields.as_str()), ("pageSize", size.as_str()), ("orderBy", "folder,name"),
            ("supportsAllDrives", "true"), ("includeItemsFromAllDrives", "true")];
        if let Some(token) = page { params.push(("pageToken", token)); }
        let http = self.session.http();
        let list: FileList = self.session.call(|token| http.json(http.client().get(format!("{API}/files")).query(&params).bearer_auth(token), true))?;
        Ok(CloudPage { entries: list.files.into_iter().map(File::entry).collect::<Result<_>>()?, next: list.next_page_token })
    }
    fn create_folder(&self, parent: &CloudId, name: &str) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        let parent = drive_id(parent)?;
        if self.child_named(parent, name)? { return Err(crate::exists()); }
        let http = self.session.http();
        let body = json!({ "name": name, "mimeType": FOLDER, "parents": [parent] });
        let file: File = self.session.call(|token| http.json(http.client().post(format!("{API}/files"))
            .query(&[("fields", FIELDS), ("supportsAllDrives", "true")]).bearer_auth(token).json(&body), false))?;
        let entry = file.entry()?;
        self.unique_after(parent, name, &entry)
    }
    fn open_download(&self, id: &CloudId, offset: u64) -> Result<Box<dyn Read + Send>> {
        let file = self.get_file(drive_id(id)?)?;
        let entry = file.entry()?;
        if entry.kind != CloudKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "Google-native documents need export, not download")); }
        let http = self.session.http(); let id = drive_id(id)?;
        let response = self.session.call(|token| {
            let mut request = http.client().get(format!("{API}/files/{id}")).query(&[("alt", "media"), ("supportsAllDrives", "true")]).bearer_auth(token);
            if offset > 0 { request = request.header(reqwest::header::RANGE, format!("bytes={offset}-")); }
            http.ok(request, true)
        })?;
        if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT { return Err(crate::protocol()); }
        Ok(Box::new(response))
    }
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        let parent_id = drive_id(parent)?;
        progress(0)?;
        if self.child_named(parent_id, name)? { return Err(crate::exists()); }
        let http = self.session.http();
        let body = json!({ "name": name, "parents": [parent_id] });
        // Session creation stores nothing; a lost reply is safe to abandon.
        let init = self.session.call(|token| http.ok(http.client().post(UPLOAD)
            .query(&[("uploadType", "resumable"), ("fields", FIELDS), ("supportsAllDrives", "true")]).bearer_auth(token)
            .header("X-Upload-Content-Type", "application/octet-stream").header("X-Upload-Content-Length", bytes.to_string()).json(&body), true))?;
        let location = init.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).ok_or_else(crate::protocol)?;
        let location = crate::http::trusted_url(location, &["www.googleapis.com"])?;
        let mut hash = Sha256::new(); let mut sent = 0u64;
        let mut buffer = Zeroizing::new(vec![0u8; CHUNK]);
        loop {
            progress(sent)?;
            let n = crate::transfer::fill(source, &mut buffer[..(bytes - sent).min(CHUNK as u64) as usize])?;
            if (n as u64) < (bytes - sent).min(CHUNK as u64) { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source shrank")); }
            hash.update(&buffer[..n]);
            let range = if bytes == 0 { "bytes */0".to_owned() } else { format!("bytes {}-{}/{}", sent, sent + n as u64 - 1, bytes) };
            let token = self.session.bearer()?;
            let response = http.send(http.client().put(location.clone()).bearer_auth(token.as_str())
                .header(reqwest::header::CONTENT_RANGE, range).body(buffer[..n].to_vec()), false)?;
            sent += n as u64;
            match response.status().as_u16() {
                308 => {
                    let confirmed = response.headers().get(reqwest::header::RANGE).and_then(|v| v.to_str().ok())
                        .and_then(|v| v.strip_prefix("bytes=0-")).and_then(|v| v.parse::<u64>().ok());
                    if confirmed != Some(sent - 1) || sent >= bytes { return Err(crate::uncertain()); }
                }
                200 | 201 => {
                    if sent != bytes { return Err(crate::uncertain()); }
                    let mut tail = [0u8; 1];
                    if source.read(&mut tail).map_err(CoreError::io)? != 0 { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source grew")); }
                    let entry = crate::http::read_json::<File>(response)?.entry()?;
                    let digest: [u8; 32] = hash.finalize().into();
                    if entry.bytes != Some(bytes) || entry.sha256.is_some_and(|s| s != digest) {
                        return Err(CoreError::new(ErrorCode::NeedsAttention, "uploaded Drive file does not match the source"));
                    }
                    progress(sent)?;
                    return self.unique_after(parent_id, name, &entry);
                }
                _ => return Err(crate::http::status_error(&response, false)),
            }
        }
    }
    /// One PATCH (atomic on the server; Drive never replaces). Duplicate names
    /// are possible on Drive, so the destination is checked before and after.
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> Result<CloudEntry> {
        if let Some(name) = new_name { crate::validate_name(name)?; }
        let file = self.current(expected)?;
        let name = new_name.unwrap_or(&file.name).to_owned();
        let old_parents = file.parents.clone().unwrap_or_default();
        let target = match new_parent { Some(p) => drive_id(p)?.to_owned(), None => old_parents.first().cloned().ok_or_else(crate::protocol)? };
        if self.child_named(&target, &name)? { return Err(crate::exists()); }
        let old = old_parents.join(",");
        let mut query: Vec<(&str, &str)> = Vec::new();
        if new_parent.is_some() { query.push(("addParents", target.as_str())); query.push(("removeParents", old.as_str())); }
        let body = if new_name.is_some() { json!({ "name": name }) } else { json!({}) };
        let entry = self.patch(drive_id(&expected.id)?, &query, body)?;
        self.unique_after(&target, &name, &entry)
    }
    fn trash(&self, expected: &CloudEntry) -> Result<()> {
        let file = self.current(expected)?;
        if file.mime_type == FOLDER && !self.list(&expected.id, None, 1)?.entries.is_empty() { return Err(crate::not_empty()); }
        self.patch(drive_id(&expected.id)?, &[], json!({ "trashed": true })).map(|_| ())
    }
}
