//! paranoic-server — the own Paranoic WebDAV server (roadmap 3.5).
//!
//! A small, auditable server for a folder on a machine you control: phone
//! backup/sync target and Cryptomator vault host (only ciphertext reaches it),
//! and the reference server for block 3 interoperability checks.
//!
//! Guarantees beyond plain WebDAV (`beneath::BeneathFs` on Linux/Android, `SafeFs` elsewhere):
//! * no path leaves the root: symlinks are invisible and never followed; on
//!   Linux/Android every operation is descriptor-relative (no race, У-23); on
//!   Windows the whole served tree must be accessible only to the owner (plus
//!   SYSTEM and Administrators), checked at start, so no other local user can
//!   plant a symlink or junction between a path check and its use (У-23);
//! * MOVE/COPY never replace an existing entry (Linux: renameat2 NOREPLACE;
//!   the dav-server check+rename race is closed); PUT + If-None-Match: * is
//!   create-new (O_EXCL);
//! * DELETE moves to a hidden server trash (`.paranoic-trash`), never erases;
//!   the trash is invisible and inaccessible over WebDAV and is emptied only
//!   by the owner: after `trash_days` or with `empty-trash` (see `trash`); authenticated
//!   responses carry `X-Paranoic-Trash: 1` so clients can confirm it (У-35);
//! * auth: random 256-bit token (Basic password or Bearer), only its BLAKE3
//!   hash is stored; constant-time compare; failed attempts are delayed;
//! * TLS required unless listening on loopback; optional read-only mode;
//!   at most 128 concurrent connections, 10 s TLS handshake and 30 s header
//!   read deadlines.
//!
//! Usage: `paranoic-server token` (prints a new token and its hash once),
//! `paranoic-server run config.json`,
//! `paranoic-server empty-trash config.json [--older-than DAYS]`.
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::Arc, time::Duration};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use dav_server::{memls::MemLs, DavHandler, DavMethodSet};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use {std::time::{SystemTime, UNIX_EPOCH}, dav_server::{davpath::DavPath, fs::*, localfs::LocalFs}, futures_util::{FutureExt, StreamExt}};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod beneath;
mod trash;

const TRASH: &str = ".paranoic-trash";
/// Concurrent connections; further clients wait in the OS accept backlog.
const MAX_CONNECTIONS: usize = 128;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);
/// Audit A35: no body data for this long ends the request.
const BODY_IDLE: Duration = Duration::from_secs(60);
/// Space left free on the served volume after an upload.
const DISK_RESERVE: u64 = 512 << 20;
static FREE_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

fn free_space(root: &std::path::Path) -> Option<u64> {
    #[cfg(unix)] { rustix::fs::statvfs(root).ok().map(|s| s.f_bavail.saturating_mul(s.f_frsize)) }
    #[cfg(not(unix))] { let _ = root; None }
}

/// Request body with an idle deadline, reset by every frame.
struct IdleBody<B> { inner: B, timer: std::pin::Pin<Box<tokio::time::Sleep>> }
impl<B> http_body::Body for IdleBody<B> where B: http_body::Body + Unpin, B::Error: std::fmt::Display {
    type Data = B::Data;
    type Error = std::io::Error;
    fn poll_frame(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
        -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use std::future::Future;
        let this = &mut *self;
        match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
            std::task::Poll::Ready(frame) => {
                this.timer.as_mut().reset(tokio::time::Instant::now() + BODY_IDLE);
                std::task::Poll::Ready(frame.map(|r| r.map_err(|e| std::io::Error::other(e.to_string()))))
            }
            std::task::Poll::Pending => if this.timer.as_mut().poll(cx).is_ready() {
                std::task::Poll::Ready(Some(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request body idle"))))
            } else { std::task::Poll::Pending },
        }
    }
    fn is_end_stream(&self) -> bool { self.inner.is_end_stream() }
    fn size_hint(&self) -> http_body::SizeHint { self.inner.size_hint() }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    root: PathBuf,
    listen: SocketAddr,
    username: String,
    /// Hex BLAKE3 of the access token (from `paranoic-server token`).
    token_blake3: String,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    #[serde(default)]
    read_only: bool,
    /// Windows: restrict the root folder to the current user (plus SYSTEM and
    /// Administrators) before serving. Without it the server refuses to start
    /// if any folder of the tree is accessible to other users. Ignored elsewhere.
    #[serde(default)]
    secure_root: bool,
    /// Remove trash buckets older than this many days (checked at start and
    /// hourly). Absent: keep everything until `empty-trash`.
    #[serde(default)]
    trash_days: Option<u32>,
}

fn fail(message: &str) -> ! { eprintln!("paranoic-server: {message}"); std::process::exit(2) }

fn read_config(path: &str) -> Config {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| fail("cannot read config"));
    let config: Config = serde_json::from_str(&text).unwrap_or_else(|_| fail("invalid config"));
    if config.trash_days == Some(0) { fail("trash_days must be at least 1 (omit it to keep the trash until empty-trash)"); }
    config
}

/// Owner command on the server machine; never reachable over WebDAV.
fn empty_trash(config: &str, older_than_days: Option<u32>) {
    let config = read_config(config);
    let root = std::fs::canonicalize(&config.root).unwrap_or_else(|_| fail("root folder unavailable"));
    match trash::purge(&root, TRASH, older_than_days.map(|days| trash::DAY * days)) {
        Ok(removed) => println!("removed {removed} trash item(s)"),
        Err(error) => fail(&format!("trash not fully emptied: {error}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["token"] => {
            let mut raw = [0u8; 32];
            raw[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            raw[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            let token = URL_SAFE_NO_PAD.encode(raw);
            println!("token (give to the client once, do not store here): {token}");
            println!("token_blake3 (put in config.json): {}", blake3::hash(token.as_bytes()).to_hex());
        }
        ["empty-trash", config] => empty_trash(config, None),
        ["empty-trash", config, "--older-than", days] => {
            let days: u32 = days.parse().unwrap_or_else(|_| fail("--older-than needs a number of days"));
            empty_trash(config, Some(days));
        }
        ["run", config] => {
            let config = read_config(config);
            let runtime = tokio::runtime::Runtime::new().unwrap_or_else(|_| fail("runtime unavailable"));
            runtime.block_on(serve(config));
        }
        _ => fail("usage: paranoic-server token | paranoic-server run <config.json>"),
    }
}

async fn serve(config: Config) {
    let root = std::fs::canonicalize(&config.root).unwrap_or_else(|_| fail("root folder unavailable"));
    if !root.is_dir() { fail("root must be a directory"); }
    #[cfg(windows)]
    windows_private_tree(&root, config.secure_root);
    #[cfg(not(windows))]
    let _ = config.secure_root;
    let trash = root.join(TRASH);
    if !trash.exists() { std::fs::create_dir(&trash).unwrap_or_else(|_| fail("cannot create trash")); }
    if let Some(days) = config.trash_days {
        let root = root.clone();
        tokio::spawn(async move {
            loop {
                let root = root.clone();
                if let Ok(Err(error)) = tokio::task::spawn_blocking(move || trash::purge(&root, TRASH, Some(trash::DAY * days))).await {
                    eprintln!("paranoic-server: trash retention: {error}");
                }
                tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            }
        });
    }
    let expected = blake3::Hash::from_hex(config.token_blake3.trim()).unwrap_or_else(|_| fail("token_blake3 must be 64 hex characters"));
    let tls = match (&config.tls_cert, &config.tls_key) {
        (Some(cert), Some(key)) => Some(tls_acceptor(cert, key)),
        (None, None) if config.listen.ip().is_loopback() => None,
        _ => fail("TLS certificate and key are required unless listening on loopback"),
    };
    // Linux/Android: descriptor-relative operations, no check-then-use race (У-23).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let fs = beneath::BeneathFs::open(&root).unwrap_or_else(|_| fail("root folder unavailable"));
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let fs = SafeFs { inner: *LocalFs::new(&root, false, false, false), root: root.clone() };
    let _ = FREE_ROOT.set(root.clone());
    let mut builder = DavHandler::builder().filesystem(Box::new(fs)).locksystem(MemLs::new()).hide_symlinks(true);
    if config.read_only { builder = builder.methods(DavMethodSet::WEBDAV_RO); }
    let handler = builder.build_handler();
    let auth = Arc::new((config.username.clone(), expected));
    // Read-only mode refuses DELETE, so it does not announce a trash.
    let announce_trash = !config.read_only;
    let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap_or_else(|_| fail("cannot listen"));
    eprintln!("paranoic-server: serving on {}{}", config.listen, if tls.is_some() { " (TLS)" } else { " (loopback, no TLS)" });
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        // Acquire a slot before accepting: bounded tasks, memory and descriptors.
        let Ok(permit) = Arc::clone(&slots).acquire_owned().await else { return };
        let Ok((stream, _)) = listener.accept().await else { continue };
        let (handler, auth, tls) = (handler.clone(), Arc::clone(&auth), tls.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |request: Request<Incoming>| {
                let (handler, auth) = (handler.clone(), Arc::clone(&auth));
                async move { Ok::<_, std::convert::Infallible>(handle(handler, &auth, announce_trash, request).await) }
            });
            let mut http = http1::Builder::new();
            http.timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT);
            match tls {
                Some(acceptor) => if let Ok(Ok(stream)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                    let _ = http.serve_connection(TokioIo::new(stream), service).await;
                },
                None => { let _ = http.serve_connection(TokioIo::new(stream), service).await; }
            }
        });
    }
}

async fn handle(handler: DavHandler, auth: &(String, blake3::Hash), announce_trash: bool, request: Request<Incoming>) -> Response<dav_server::body::Body> {
    if !authorized(auth, request.headers().get(hyper::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut response = Response::new(dav_server::body::Body::from(String::new()));
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        response.headers_mut().insert(hyper::header::WWW_AUTHENTICATE, hyper::header::HeaderValue::from_static("Basic realm=\"Paranoic\", charset=\"UTF-8\""));
        return response;
    }
    let mut request = request;
    force_no_overwrite(&mut request);
    // Audit A35: refuse an upload that would leave the disk nearly full (trash included).
    if let Some(length) = request.headers().get(hyper::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()) {
        if let Some(free) = free_space(&FREE_ROOT.get().cloned().unwrap_or_default()) {
            if length.saturating_add(DISK_RESERVE) > free {
                let mut response = Response::new(dav_server::body::Body::from(String::new()));
                *response.status_mut() = StatusCode::INSUFFICIENT_STORAGE;
                return response;
            }
        }
    }
    // Audit A35: a body that stops arriving ends the request instead of holding a slot.
    let request = request.map(|body| IdleBody { inner: body, timer: Box::pin(tokio::time::sleep(BODY_IDLE)) });
    let mut response = handler.handle(request).await;
    if announce_trash { response.headers_mut().insert("x-paranoic-trash", hyper::header::HeaderValue::from_static("1")); }
    response
}

/// MOVE/COPY never replace (module docs). WebDAV defaults to `Overwrite: T`, in which
/// case dav-server first DELETEs an existing destination (into the trash here) and
/// only then calls the no-replace rename. Any client — not only Paranoic's own,
/// which already sends `F` — therefore gets `Overwrite: F`: an existing destination
/// is answered with 412 Precondition Failed and nothing is removed.
fn force_no_overwrite<B>(request: &mut Request<B>) {
    let method = request.method().as_str();
    if method == "MOVE" || method == "COPY" {
        request.headers_mut().insert("overwrite", hyper::header::HeaderValue::from_static("F"));
    }
}

fn authorized((username, expected): &(String, blake3::Hash), header: Option<&str>) -> bool {
    let Some(header) = header else { return false };
    let token = if let Some(bearer) = header.strip_prefix("Bearer ") { bearer.to_owned() }
        else if let Some(basic) = header.strip_prefix("Basic ") {
            let Ok(decoded) = STANDARD.decode(basic.trim()) else { return false };
            let Ok(text) = String::from_utf8(decoded) else { return false };
            let Some((user, password)) = text.split_once(':') else { return false };
            if user != username { return false; }
            password.to_owned()
        } else { return false };
    // blake3::Hash equality is constant-time.
    blake3::hash(token.as_bytes()) == *expected
}

fn tls_acceptor(cert: &Path, key: &Path) -> tokio_rustls::TlsAcceptor {
    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(cert).unwrap_or_else(|_| fail("cannot read TLS certificate"))))
        .collect::<Result<_, _>>().unwrap_or_else(|_| fail("invalid TLS certificate"));
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(std::fs::File::open(key).unwrap_or_else(|_| fail("cannot read TLS key"))))
        .ok().flatten().unwrap_or_else(|| fail("invalid TLS key"));
    let config = tokio_rustls::rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)
        .unwrap_or_else(|_| fail("TLS configuration rejected"));
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

/// Other hosts: LocalFs with no-replace renames/copies, trash instead of
/// deletion and an unreachable trash folder. The path checks below are
/// check-then-use; on Windows that window is closed by access control
/// (`windows_private_tree`): only the owner, SYSTEM and Administrators can
/// change the tree. Other non-Linux hosts keep the residual race (У-23).
#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[derive(Clone)]
struct SafeFs { inner: LocalFs, root: PathBuf }
#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl SafeFs {
    fn hidden(path: &DavPath) -> bool {
        path.as_rel_ospath().components().next().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(TRASH))
    }
    fn os(&self, path: &DavPath) -> PathBuf { self.root.join(path.as_rel_ospath()) }
    /// Refuse paths that pass through a symlink anywhere below the root
    /// (hide_symlinks only hides them from listings; LocalFs would follow
    /// them, also out of the root). Residual: check-then-use race against a
    /// local user who can change the tree on the host (register У-23).
    fn confined(&self, path: &DavPath) -> bool {
        let mut current = self.root.clone();
        for part in path.as_rel_ospath().components() {
            match part {
                std::path::Component::Normal(name) => current.push(name),
                _ => return false,
            }
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => return false,
                Ok(_) => {}
                Err(_) => return true, // the rest does not exist yet (create)
            }
        }
        true
    }
    fn refused(&self, path: &DavPath) -> bool { Self::hidden(path) || !self.confined(path) }
    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> FsResult<T> {
        tokio::task::spawn_blocking(f).await.map_err(|_| FsError::GeneralFailure)?.map_err(FsError::from)
    }
    async fn to_trash(&self, path: &DavPath) -> FsResult<()> {
        if self.refused(path) || path.as_rel_ospath().as_os_str().is_empty() { return Err(FsError::Forbidden); }
        let source = self.os(path);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let bucket = self.root.join(TRASH).join(format!("{stamp}-{}", uuid::Uuid::new_v4().simple()));
        let name = source.file_name().map(|n| n.to_owned()).ok_or(FsError::Forbidden)?;
        Self::blocking(move || { std::fs::create_dir(&bucket)?; rename_no_replace(&source, &bucket.join(name)) }).await
    }
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    { rustix::fs::renameat_with(rustix::fs::CWD, from, rustix::fs::CWD, to, rustix::fs::RenameFlags::NOREPLACE).map_err(std::io::Error::from) }
    // Windows: one MoveFileExW without MOVEFILE_REPLACE_EXISTING (atomic no-replace).
    #[cfg(windows)]
    { paranoic_windows::rename_no_replace(from, to) }
    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
    {
        // Residual race on other hosts (check, then rename).
        if std::fs::symlink_metadata(to).is_ok() { return Err(std::io::ErrorKind::AlreadyExists.into()); }
        std::fs::rename(from, to)
    }
}
/// Windows (У-23): refuse to serve a tree that another local user could
/// change. Every folder must pass `is_owner_only` (owner: this user, SYSTEM or
/// Administrators; allow entries only for them). Folder links and junctions
/// are not entered (requests through them are refused anyway). With
/// `secure_root` the root ACL is replaced first; folders with their own
/// explicit entries still have to be fixed by hand.
#[cfg(windows)]
fn windows_private_tree(root: &Path, secure_root: bool) {
    if secure_root && paranoic_windows::ensure_owner_only(root).is_err() { fail("cannot restrict the root folder to the current user"); }
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(folder) = stack.pop() {
        match paranoic_windows::is_owner_only(&folder) {
            Ok(true) => {}
            Ok(false) => fail(&format!("folder is accessible to other users: {} (set \"secure_root\": true or restrict its permissions)", folder.display())),
            Err(_) => fail(&format!("cannot read permissions of {}", folder.display())),
        }
        let entries = std::fs::read_dir(&folder).unwrap_or_else(|_| fail(&format!("cannot list {}", folder.display())));
        // Every listing or classification error stops the start (audit 19):
        // an unchecked branch must not be served.
        for entry in entries {
            let entry = entry.unwrap_or_else(|_| fail(&format!("cannot list {}", folder.display())));
            seen += 1;
            if seen > 1_000_000 { fail("served tree too large to verify"); }
            let kind = entry.file_type().unwrap_or_else(|_| fail(&format!("cannot classify an entry in {}", folder.display())));
            if kind.is_dir() && !kind.is_symlink() { stack.push(entry.path()); }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn copy_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if !std::fs::symlink_metadata(from)?.is_file() { return Err(std::io::ErrorKind::Unsupported.into()); }
    let mut source = std::fs::File::open(from)?;
    // Complete copy under a hidden temporary name, then a hard link to the
    // final name (fails if it exists) — never a partial file there (audit 10).
    let parent = to.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let temporary = parent.join(format!(".paranoic-copy-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut target = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()?;
        drop(target);
        std::fs::hard_link(&temporary, to)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl DavFileSystem for SafeFs {
    fn open<'a>(&'a self, path: &'a DavPath, options: OpenOptions) -> FsFuture<'a, Box<dyn DavFile>> {
        if self.refused(path) { return async { Err(FsError::NotFound) }.boxed(); }
        DavFileSystem::open(&self.inner, path, options)
    }
    fn read_dir<'a>(&'a self, path: &'a DavPath, meta: ReadDirMeta) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        async move {
            if self.refused(path) { return Err(FsError::NotFound); }
            let top = path.as_rel_ospath().as_os_str().is_empty();
            let stream = DavFileSystem::read_dir(&self.inner, path, meta).await?;
            let filtered = stream.filter(move |entry| {
                let keep = !(top && entry.as_ref().is_ok_and(|e| String::from_utf8_lossy(&e.name()).eq_ignore_ascii_case(TRASH)));
                async move { keep }
            });
            Ok(Box::pin(filtered) as FsStream<Box<dyn DavDirEntry>>)
        }.boxed()
    }
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        if self.refused(path) { return async { Err(FsError::NotFound) }.boxed(); }
        DavFileSystem::metadata(&self.inner, path)
    }
    fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        if self.refused(path) { return async { Err(FsError::NotFound) }.boxed(); }
        DavFileSystem::symlink_metadata(&self.inner, path)
    }
    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        if self.refused(path) { return async { Err(FsError::Forbidden) }.boxed(); }
        DavFileSystem::create_dir(&self.inner, path)
    }
    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> { self.to_trash(path).boxed() }
    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> { self.to_trash(path).boxed() }
    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            if self.refused(from) || self.refused(to) { return Err(FsError::Forbidden); }
            let (from, to) = (self.os(from), self.os(to));
            Self::blocking(move || rename_no_replace(&from, &to)).await
        }.boxed()
    }
    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            if self.refused(from) || self.refused(to) { return Err(FsError::Forbidden); }
            let (from, to) = (self.os(from), self.os(to));
            Self::blocking(move || copy_no_replace(&from, &to)).await
        }.boxed()
    }
    fn get_quota(&self) -> FsFuture<'_, (u64, Option<u64>)> { DavFileSystem::get_quota(&self.inner) }
}

#[cfg(test)]
mod overwrite_tests {
    use super::force_no_overwrite;
    use hyper::Request;

    #[test] fn move_and_copy_never_overwrite() {
        for method in ["MOVE", "COPY"] {
            let mut request = Request::builder().method(method).header("overwrite", "T").body(()).unwrap();
            force_no_overwrite(&mut request);
            assert_eq!(request.headers()["overwrite"], "F", "{method}");
            let mut missing = Request::builder().method(method).body(()).unwrap();
            force_no_overwrite(&mut missing);
            assert_eq!(missing.headers()["overwrite"], "F", "{method} without header defaults to T in WebDAV");
        }
        let mut put = Request::builder().method("PUT").body(()).unwrap();
        force_no_overwrite(&mut put);
        assert!(put.headers().get("overwrite").is_none());
    }
}
