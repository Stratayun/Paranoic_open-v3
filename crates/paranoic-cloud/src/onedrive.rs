//! OneDrive / Microsoft Graph v1.0. Name conflicts are rejected server-side
//! (conflictBehavior=fail) and mutations carry If-Match with the item eTag.
//! Upload-session and download URLs are pre-authenticated: no bearer is sent.
use std::{io::Read, sync::Arc};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CloudCapabilities, CloudEntry, CloudId, CloudKind, CloudPage, CloudProvider, CloudSession, CoreError, ErrorCode,
    ProviderKind, Quota, Result};

const API: &str = "https://graph.microsoft.com/v1.0/me/drive";
const SELECT: &str = "id,name,size,lastModifiedDateTime,eTag,file,folder,parentReference,package";
const CHUNK: usize = 10 << 20; // 32 × 320 KiB
const CONTENT_HOSTS: [&str; 6] = [".sharepoint.com", "api.onedrive.com", ".1drv.com", ".onedrive.com", ".livefilestore.com", ".microsoftpersonalcontent.com"];

pub struct OneDrive { session: Arc<CloudSession> }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item { id: String, name: String, size: Option<u64>, last_modified_date_time: Option<String>,
    #[serde(rename = "eTag")] e_tag: Option<String>, file: Option<FileFacet>, folder: Option<FolderFacet>,
    parent_reference: Option<ParentRef>, package: Option<serde_json::Value> }
#[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct FileFacet { hashes: Option<Hashes> }
#[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct Hashes { sha256_hash: Option<String>, quick_xor_hash: Option<String> }
#[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct FolderFacet { child_count: Option<u64> }
#[derive(Deserialize)] struct ParentRef { id: Option<String> }
#[derive(Deserialize)]
struct Children { value: Vec<Item>, #[serde(rename = "@odata.nextLink")] next: Option<String> }

impl Item {
    fn entry(self) -> Result<CloudEntry> {
        let kind = if self.folder.is_some() { CloudKind::Folder } else if self.file.is_some() { CloudKind::File } else { CloudKind::Native };
        let _ = &self.package;
        Ok(CloudEntry { id: CloudId::parse(self.id)?, name: self.name, kind,
            bytes: if kind == CloudKind::File { Some(self.size.unwrap_or(0)) } else { None },
            modified_unix_ms: self.last_modified_date_time.as_deref().and_then(crate::rfc3339_ms),
            revision: self.e_tag,
            sha256: self.file.as_ref().and_then(|f| f.hashes.as_ref()).and_then(|h| h.sha256_hash.as_deref()).and_then(|h| crate::hex32(&h.to_ascii_lowercase())),
            quick_xor: self.file.and_then(|f| f.hashes).and_then(|h| h.quick_xor_hash),
            parents: self.parent_reference.and_then(|p| p.id).map(CloudId::parse).transpose()?.into_iter().collect() })
    }
}

fn item_url(id: &CloudId, suffix: &[&str]) -> Result<Url> {
    let mut url = Url::parse(API).map_err(|_| crate::protocol())?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| crate::protocol())?;
        segments.push("items").push(id.as_str());
        for s in suffix { segments.push(s); }
    }
    Ok(url)
}

impl OneDrive {
    pub fn new(session: Arc<CloudSession>) -> Result<Self> {
        if session.provider() != ProviderKind::OneDrive { return Err(CoreError::new(ErrorCode::InvalidInput, "session is not for OneDrive")); }
        Ok(Self { session })
    }
    fn get(&self, url: Url) -> Result<Item> {
        let http = self.session.http();
        self.session.call(|token| http.json(http.client().get(url.clone()).query(&[("$select", SELECT)]).bearer_auth(token), true))
    }
    fn patch(&self, expected: &CloudEntry, body: serde_json::Value) -> Result<CloudEntry> {
        let etag = expected.revision.as_deref().ok_or_else(crate::stale)?;
        let http = self.session.http(); let url = item_url(&expected.id, &[])?;
        let item: Item = self.session.call(|token| http.json(http.client().patch(url.clone()).bearer_auth(token)
            .header(reqwest::header::IF_MATCH, etag).json(&body), false))?;
        item.entry()
    }
}

impl CloudProvider for OneDrive {
    fn kind(&self) -> ProviderKind { ProviderKind::OneDrive }
    fn capabilities(&self) -> CloudCapabilities {
        CloudCapabilities { resumable_upload: true, ranged_download: true, conditional_mutation: true, atomic_create_new: true,
            duplicate_names: false, server_sha256: false, trash: true, atomic_relocate: true }
    }
    fn quota(&self) -> Result<Quota> {
        #[derive(Deserialize)] struct Drive { quota: Option<Q> }
        #[derive(Deserialize)] struct Q { used: Option<u64>, total: Option<u64> }
        let http = self.session.http();
        let drive: Drive = self.session.call(|token| http.json(http.client().get(API).query(&[("$select", "quota")]).bearer_auth(token), true))?;
        Ok(drive.quota.map(|q| Quota { used: q.used, total: q.total }).unwrap_or_default())
    }
    fn account_id(&self) -> Result<String> {
        #[derive(Deserialize)] struct Drive { id: String }
        let http = self.session.http();
        let drive: Drive = self.session.call(|token| http.json(http.client().get(API).query(&[("$select", "id")]).bearer_auth(token), true))?;
        Ok(drive.id)
    }
    fn lookup(&self, parent: &CloudId, name: &str) -> Result<Option<CloudEntry>> {
        crate::validate_name(name)?;
        if name.contains(':') || parent.as_str().contains(':') { return Err(CoreError::new(ErrorCode::InvalidPath, "invalid OneDrive name")); }
        let mut url = Url::parse(API).map_err(|_| crate::protocol())?;
        url.path_segments_mut().map_err(|_| crate::protocol())?.push("items").push(&format!("{}:", parent.as_str())).push(name);
        match self.get(url) {
            Ok(item) => item.entry().map(Some),
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn root(&self) -> Result<CloudEntry> { self.get(Url::parse(&format!("{API}/root")).map_err(|_| crate::protocol())?)?.entry() }
    fn metadata(&self, id: &CloudId) -> Result<CloudEntry> { self.get(item_url(id, &[])?)?.entry() }
    fn list(&self, folder: &CloudId, page: Option<&str>, limit: usize) -> Result<CloudPage> {
        if !(1..=1000).contains(&limit) { return Err(CoreError::new(ErrorCode::InvalidInput, "page limit out of range")); }
        let url = match page {
            Some(next) => crate::http::trusted_url(next, &["graph.microsoft.com"])?,
            None => { let mut url = item_url(folder, &["children"])?;
                url.query_pairs_mut().append_pair("$top", &limit.to_string()).append_pair("$select", SELECT); url }
        };
        let http = self.session.http();
        let children: Children = self.session.call(|token| http.json(http.client().get(url.clone()).bearer_auth(token), true))?;
        Ok(CloudPage { entries: children.value.into_iter().map(Item::entry).collect::<Result<_>>()?, next: children.next })
    }
    fn create_folder(&self, parent: &CloudId, name: &str) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        let http = self.session.http(); let url = item_url(parent, &["children"])?;
        let body = json!({ "name": name, "folder": {}, "@microsoft.graph.conflictBehavior": "fail" });
        let item: Item = self.session.call(|token| http.json(http.client().post(url.clone()).bearer_auth(token).json(&body), false))?;
        item.entry()
    }
    fn open_download(&self, id: &CloudId, offset: u64) -> Result<Box<dyn Read + Send>> {
        let http = self.session.http(); let url = item_url(id, &["content"])?;
        let response = self.session.call(|token| http.send(http.client().get(url.clone()).bearer_auth(token), true))?;
        if !response.status().is_redirection() {
            return Err(if response.status().is_success() { crate::protocol() } else { crate::http::status_error(&response, true) });
        }
        let location = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).ok_or_else(crate::protocol)?;
        let target = crate::http::trusted_url(location, &CONTENT_HOSTS)?;
        let mut request = http.client().get(target);
        if offset > 0 { request = request.header(reqwest::header::RANGE, format!("bytes={offset}-")); }
        let content = http.ok(request, true)?;
        if offset > 0 && content.status() != reqwest::StatusCode::PARTIAL_CONTENT { return Err(crate::protocol()); }
        Ok(Box::new(content))
    }
    fn upload_new(&self, parent: &CloudId, name: &str, bytes: u64, source: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> Result<()>) -> Result<CloudEntry> {
        crate::validate_name(name)?;
        progress(0)?;
        let http = self.session.http();
        if bytes == 0 {
            let mut url = child_url(parent, name, "content")?;
            url.query_pairs_mut().append_pair("@microsoft.graph.conflictBehavior", "fail");
            let item: Item = self.session.call(|token| http.json(http.client().put(url.clone()).bearer_auth(token).body(Vec::new()), false))?;
            return item.entry();
        }
        let url = child_url(parent, name, "createUploadSession")?;
        #[derive(Deserialize)] #[serde(rename_all = "camelCase")] struct Session { upload_url: String }
        let body = json!({ "item": { "@microsoft.graph.conflictBehavior": "fail" } });
        // Creating a session stores nothing user-visible; safe to abandon.
        let session: Session = self.session.call(|token| http.json(http.client().post(url.clone()).bearer_auth(token).json(&body), true))?;
        let upload = crate::http::trusted_url(&session.upload_url, &CONTENT_HOSTS)?;
        let mut hash = Sha256::new(); let mut sent = 0u64;
        let mut buffer = Zeroizing::new(vec![0u8; CHUNK]);
        let result = (|| loop {
            progress(sent)?;
            let want = (bytes - sent).min(CHUNK as u64) as usize;
            let n = crate::transfer::fill(source, &mut buffer[..want])?;
            if n < want { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source shrank")); }
            hash.update(&buffer[..n]);
            let response = http.send(http.client().put(upload.clone())
                .header(reqwest::header::CONTENT_RANGE, format!("bytes {}-{}/{}", sent, sent + n as u64 - 1, bytes))
                .body(buffer[..n].to_vec()), false)?;
            sent += n as u64;
            match response.status().as_u16() {
                202 if sent < bytes => {}
                200 | 201 if sent == bytes => {
                    let mut tail = [0u8; 1];
                    if source.read(&mut tail).map_err(CoreError::io)? != 0 { return Err(CoreError::new(ErrorCode::SourceChanged, "upload source grew")); }
                    let entry = crate::http::read_json::<Item>(response)?.entry()?;
                    let digest: [u8; 32] = hash.clone().finalize().into();
                    if entry.bytes != Some(bytes) || entry.sha256.is_some_and(|s| s != digest) {
                        return Err(CoreError::new(ErrorCode::NeedsAttention, "uploaded OneDrive file does not match the source"));
                    }
                    progress(sent)?;
                    return Ok(entry);
                }
                409 => return Err(crate::exists()),
                _ if response.status().is_success() => return Err(crate::uncertain()),
                _ => return Err(crate::http::status_error(&response, false)),
            }
        })();
        if result.is_err() { let _ = http.send(http.client().delete(upload), true); } // cancel session, best effort
        result
    }
    /// One PATCH with If-Match; Graph rejects an existing name (409), never replaces.
    fn relocate(&self, expected: &CloudEntry, new_parent: Option<&CloudId>, new_name: Option<&str>) -> Result<CloudEntry> {
        let mut body = serde_json::Map::new();
        if let Some(name) = new_name { crate::validate_name(name)?; body.insert("name".into(), json!(name)); }
        if let Some(parent) = new_parent { body.insert("parentReference".into(), json!({ "id": parent.as_str() })); }
        body.insert("@microsoft.graph.conflictBehavior".into(), json!("fail"));
        self.patch(expected, serde_json::Value::Object(body))
    }
    fn trash(&self, expected: &CloudEntry) -> Result<()> {
        let current = self.get(item_url(&expected.id, &[])?)?;
        if current.e_tag != expected.revision { return Err(crate::stale()); }
        if current.folder.as_ref().is_some_and(|f| f.child_count != Some(0)) { return Err(crate::not_empty()); }
        let etag = expected.revision.as_deref().ok_or_else(crate::stale)?;
        let http = self.session.http(); let url = item_url(&expected.id, &[])?;
        self.session.call(|token| http.ok(http.client().delete(url.clone()).bearer_auth(token).header(reqwest::header::IF_MATCH, etag), false)).map(|_| ())
    }
}

/// Graph path addressing `items/{parent}:/{name}:/{action}`; the segment
/// builder percent-encodes the name. ResourcePath already forbids ':'.
fn child_url(parent: &CloudId, name: &str, action: &str) -> Result<Url> {
    if name.contains(':') || parent.as_str().contains(':') { return Err(CoreError::new(ErrorCode::InvalidPath, "invalid OneDrive name")); }
    let mut url = Url::parse(API).map_err(|_| crate::protocol())?;
    url.path_segments_mut().map_err(|_| crate::protocol())?
        .push("items").push(&format!("{}:", parent.as_str())).push(&format!("{name}:")).push(action);
    Ok(url)
}
