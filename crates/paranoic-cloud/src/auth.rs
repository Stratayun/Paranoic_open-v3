//! OAuth 2.0 for native apps (RFC 8252): system browser, PKCE S256 where the
//! provider supports it, state check, loopback redirect for Windows, custom
//! scheme/app link redirect for Android (the host hands back the URL).
//! Tokens live in zeroizing memory; persistence is sealed (see tokens.rs).
use std::{io::{Read, Write}, net::TcpListener, sync::{atomic::{AtomicBool, Ordering}, Mutex}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Http, ProviderKind, Result};

const EXPIRED: &str = "cloud session expired or revoked";
const PCLOUD_HOSTS: [&str; 2] = ["api.pcloud.com", "eapi.pcloud.com"];

/// Registered application. Desktop/installed-app "secrets" are not secret;
/// pCloud requires one, Google desktop clients may, Microsoft public clients do not.
pub struct OAuthClient {
    pub provider: ProviderKind,
    pub client_id: String,
    pub client_secret: Option<Zeroizing<String>>,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}
impl OAuthClient {
    /// Scope `drive` lets the user browse existing files; `drive.file` limits to app-created ones.
    pub fn google(client_id: impl Into<String>, client_secret: Option<Zeroizing<String>>, redirect_uri: impl Into<String>, full_drive: bool) -> Self {
        let scope = if full_drive { "https://www.googleapis.com/auth/drive" } else { "https://www.googleapis.com/auth/drive.file" };
        Self { provider: ProviderKind::GoogleDrive, client_id: client_id.into(), client_secret, redirect_uri: redirect_uri.into(), scopes: vec![scope.into()] }
    }
    pub fn onedrive(client_id: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self { provider: ProviderKind::OneDrive, client_id: client_id.into(), client_secret: None, redirect_uri: redirect_uri.into(),
            scopes: vec!["Files.ReadWrite".into(), "User.Read".into(), "offline_access".into()] }
    }
    pub fn pcloud(client_id: impl Into<String>, client_secret: Zeroizing<String>, redirect_uri: impl Into<String>) -> Self {
        Self { provider: ProviderKind::PCloud, client_id: client_id.into(), client_secret: Some(client_secret), redirect_uri: redirect_uri.into(), scopes: Vec::new() }
    }
    fn authorize_endpoint(&self) -> &'static str {
        match self.provider { ProviderKind::GoogleDrive => "https://accounts.google.com/o/oauth2/v2/auth",
            ProviderKind::OneDrive => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            ProviderKind::PCloud => "https://my.pcloud.com/oauth2/authorize", ProviderKind::WebDav => "" }
    }
    fn token_endpoint(&self) -> &'static str {
        match self.provider { ProviderKind::GoogleDrive => "https://oauth2.googleapis.com/token",
            ProviderKind::OneDrive => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            ProviderKind::PCloud | ProviderKind::WebDav => "" }
    }

    /// Build the browser URL. Keep the returned request in memory only.
    pub fn begin(&self) -> Result<AuthRequest> {
        if self.provider == ProviderKind::WebDav { return Err(CoreError::new(ErrorCode::Unsupported, "WebDAV uses server credentials, not OAuth")); }
        let state = Zeroizing::new(random_token(16)?);
        let verifier = Zeroizing::new(random_token(32)?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut params: Vec<(&str, String)> = vec![("client_id", self.client_id.clone()), ("response_type", "code".into()),
            ("redirect_uri", self.redirect_uri.clone()), ("state", state.to_string())];
        match self.provider {
            ProviderKind::PCloud | ProviderKind::WebDav => {}
            ProviderKind::GoogleDrive => {
                params.extend([("scope", self.scopes.join(" ")), ("code_challenge", challenge), ("code_challenge_method", "S256".into()),
                    ("access_type", "offline".into()), ("prompt", "consent".into())]);
            }
            ProviderKind::OneDrive => {
                params.extend([("scope", self.scopes.join(" ")), ("code_challenge", challenge), ("code_challenge_method", "S256".into()),
                    ("response_mode", "query".into())]);
            }
        }
        let url = Url::parse_with_params(self.authorize_endpoint(), &params).map_err(|_| invalid())?;
        Ok(AuthRequest { url: url.to_string(), state, verifier })
    }

    /// Complete login from the redirect URL delivered to the app.
    pub fn finish(&self, http: &Http, request: AuthRequest, redirect: &str) -> Result<TokenSet> {
        let url = Url::parse(redirect).map_err(|_| invalid())?;
        let expected = Url::parse(&self.redirect_uri).map_err(|_| invalid())?;
        if url.scheme() != expected.scheme() || url.host_str() != expected.host_str() || url.port_or_known_default() != expected.port_or_known_default()
            || url.path() != expected.path() { return Err(invalid()); }
        let mut code = None; let mut state = None; let mut hostname = None; let mut failed = false;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(Zeroizing::new(value.into_owned())),
                "state" => state = Some(Zeroizing::new(value.into_owned())),
                "hostname" => hostname = Some(value.into_owned()),
                "error" => failed = true,
                _ => {}
            }
        }
        if failed { return Err(CoreError::new(ErrorCode::PermissionDenied, "cloud login was declined")); }
        let (Some(code), Some(state)) = (code, state) else { return Err(invalid()); };
        if !constant_time_eq(state.as_bytes(), request.state.as_bytes()) { return Err(CoreError::new(ErrorCode::PermissionDenied, "cloud login state mismatch")); }
        if self.provider == ProviderKind::PCloud {
            let host = hostname.unwrap_or_else(|| "api.pcloud.com".into());
            if !PCLOUD_HOSTS.contains(&host.as_str()) { return Err(invalid()); }
            let secret = self.client_secret.as_ref().ok_or_else(invalid)?;
            #[derive(Deserialize)] struct Reply { result: i64, access_token: Option<String> }
            let reply: Reply = http.json(http.client().get(format!("https://{host}/oauth2_token"))
                .query(&[("client_id", self.client_id.as_str()), ("client_secret", secret.as_str()), ("code", code.as_str())]), true)?;
            let access = reply.access_token.filter(|_| reply.result == 0).ok_or_else(|| CoreError::new(ErrorCode::PermissionDenied, "pCloud login failed"))?;
            return Ok(TokenSet { access: Zeroizing::new(access), refresh: None, expires_at_unix: None, api_host: Some(host) });
        }
        let mut form: Vec<(&str, &str)> = vec![("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", &self.redirect_uri),
            ("client_id", &self.client_id), ("code_verifier", request.verifier.as_str())];
        if let Some(secret) = &self.client_secret { form.push(("client_secret", secret.as_str())); }
        self.token_request(http, &form, None)
    }

    fn token_request(&self, http: &Http, form: &[(&str, &str)], previous_refresh: Option<&Zeroizing<String>>) -> Result<TokenSet> {
        #[derive(Deserialize)] struct Reply { access_token: String, refresh_token: Option<String>, expires_in: Option<u64> }
        let response = http.send(http.client().post(self.token_endpoint()).form(form), true)?;
        if response.status().is_client_error() { return Err(CoreError::new(ErrorCode::PermissionDenied, EXPIRED)); }
        if !response.status().is_success() { return Err(crate::http::status_error(&response, true)); }
        let reply: Reply = crate::http::read_json(response)?;
        let refresh = reply.refresh_token.map(Zeroizing::new).or_else(|| previous_refresh.cloned());
        Ok(TokenSet { access: Zeroizing::new(reply.access_token), refresh,
            expires_at_unix: reply.expires_in.map(|s| now().saturating_add(s)), api_host: None })
    }

    pub fn refresh(&self, http: &Http, tokens: &TokenSet) -> Result<TokenSet> {
        let refresh = tokens.refresh.as_ref().ok_or_else(|| CoreError::new(ErrorCode::PermissionDenied, EXPIRED))?;
        let mut form: Vec<(&str, &str)> = vec![("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("client_id", &self.client_id)];
        if let Some(secret) = &self.client_secret { form.push(("client_secret", secret.as_str())); }
        let scope = self.scopes.join(" ");
        if self.provider == ProviderKind::OneDrive { form.push(("scope", &scope)); }
        let mut next = self.token_request(http, &form, Some(refresh))?;
        next.api_host = tokens.api_host.clone();
        Ok(next)
    }
}

/// Pending login. Not serializable: a restart requires a new login attempt.
pub struct AuthRequest { pub url: String, state: Zeroizing<String>, verifier: Zeroizing<String> }

#[derive(Clone)]
pub struct TokenSet {
    pub access: Zeroizing<String>,
    pub refresh: Option<Zeroizing<String>>,
    pub expires_at_unix: Option<u64>,
    /// pCloud regional API host (api.pcloud.com / eapi.pcloud.com).
    pub api_host: Option<String>,
}

type Persist = Box<dyn Fn(&TokenSet) -> Result<()> + Send + Sync>;

/// Live authorized session shared by one provider adapter.
/// Tokens are held under a short-lived lock only; network refreshes run
/// outside it (serialized by `refreshing`), so `lock()` never waits for the
/// network and a refresh finishing after lock() is discarded. If rotated
/// tokens cannot be persisted they stay in memory (the old refresh token may
/// already be revoked) and `tokens_unsaved()` reports it.
pub struct CloudSession {
    http: Http, client: OAuthClient, tokens: Mutex<Option<TokenSet>>, persist: Persist,
    refreshing: Mutex<()>, locked: AtomicBool, unsaved: AtomicBool,
}
impl CloudSession {
    /// `persist` receives rotated tokens (seal them with `seal_tokens`).
    pub fn new(http: Http, client: OAuthClient, tokens: TokenSet, persist: Persist) -> Self {
        Self { http, client, tokens: Mutex::new(Some(tokens)), persist, refreshing: Mutex::new(()),
            locked: AtomicBool::new(false), unsaved: AtomicBool::new(false) }
    }
    pub fn provider(&self) -> ProviderKind { self.client.provider }
    pub(crate) fn http(&self) -> &Http { &self.http }
    pub(crate) fn api_host(&self) -> Result<String> {
        let guard = self.tokens.lock().map_err(|_| poisoned())?;
        let host = guard.as_ref().and_then(|t| t.api_host.clone()).unwrap_or_else(|| "api.pcloud.com".into());
        if PCLOUD_HOSTS.contains(&host.as_str()) { Ok(host) } else { Err(invalid()) }
    }
    /// Quick Lock / logout: forget tokens in memory immediately (no network
    /// wait). Stored sealed tokens are the host's responsibility.
    pub fn lock(&self) {
        self.locked.store(true, Ordering::Release);
        let mut guard = match self.tokens.lock() { Ok(g) => g, Err(e) => e.into_inner() };
        guard.take();
    }
    /// Rotated tokens are only in memory (persist failed); retry with `retry_persist`.
    pub fn tokens_unsaved(&self) -> bool { self.unsaved.load(Ordering::Acquire) }
    pub fn retry_persist(&self) -> Result<()> {
        let snapshot = self.tokens.lock().map_err(|_| poisoned())?.clone().ok_or_else(locked)?;
        (self.persist)(&snapshot)?;
        self.unsaved.store(false, Ordering::Release);
        Ok(())
    }
    fn snapshot(&self) -> Result<TokenSet> {
        if self.locked.load(Ordering::Acquire) { return Err(locked()); }
        self.tokens.lock().map_err(|_| poisoned())?.clone().ok_or_else(locked)
    }
    /// Refresh outside the token lock; store only if not locked meanwhile.
    fn refresh_from(&self, current: &TokenSet) -> Result<()> {
        let _serial = self.refreshing.lock().map_err(|_| poisoned())?;
        // Another caller may have refreshed while we waited.
        let latest = self.snapshot()?;
        if latest.access.as_str() != current.access.as_str() { return Ok(()); }
        let next = self.client.refresh(&self.http, &latest)?;
        {
            let mut guard = self.tokens.lock().map_err(|_| poisoned())?;
            if self.locked.load(Ordering::Acquire) || guard.is_none() { return Err(locked()); } // drop `next`
            *guard = Some(next.clone());
        }
        match (self.persist)(&next) {
            Ok(()) => self.unsaved.store(false, Ordering::Release),
            Err(_) => self.unsaved.store(true, Ordering::Release),
        }
        Ok(())
    }

    /// Current access token, refreshed when it expires within two minutes.
    pub fn bearer(&self) -> Result<Zeroizing<String>> {
        let current = self.snapshot()?;
        if current.expires_at_unix.is_some_and(|t| t <= now().saturating_add(120)) && current.refresh.is_some() {
            self.refresh_from(&current)?;
        }
        Ok(self.snapshot()?.access)
    }
    fn force_refresh(&self) -> Result<()> {
        let current = self.snapshot()?;
        self.refresh_from(&current)
    }
    /// Run an authorized call; after a 401 (never applied by the server)
    /// refresh once and repeat. Other errors are returned unchanged.
    pub(crate) fn call<T>(&self, mut f: impl FnMut(&str) -> Result<T>) -> Result<T> {
        let token = self.bearer()?;
        match f(&token) {
            Err(error) if error.message == EXPIRED && self.client.provider != ProviderKind::PCloud => {
                self.force_refresh()?;
                let token = self.bearer()?;
                f(&token)
            }
            other => other,
        }
    }
}

/// One-shot loopback receiver for the Windows browser redirect.
pub struct LoopbackRedirect { listener: TcpListener, port: u16 }
impl LoopbackRedirect {
    pub fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(CoreError::io)?;
        let port = listener.local_addr().map_err(CoreError::io)?.port();
        listener.set_nonblocking(true).map_err(CoreError::io)?;
        Ok(Self { listener, port })
    }
    pub fn redirect_uri(&self) -> String { format!("http://127.0.0.1:{}/callback", self.port) }
    /// Wait for the browser; returns the full redirect URL for `finish`.
    /// Only a callback carrying this request's `state` ends the wait; other
    /// local requests get 400 and are ignored. The page never claims success:
    /// the code exchange happens afterwards in `finish`.
    pub fn wait(self, pending: &AuthRequest, timeout: Duration, mut check: impl FnMut() -> Result<()>) -> Result<String> {
        let until = Instant::now().checked_add(timeout).ok_or_else(invalid)?;
        let remaining = || until.checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| CoreError::new(ErrorCode::Cancelled, "cloud login timed out"));
        loop {
            check()?;
            remaining()?;
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).map_err(CoreError::io)?;
                    let mut buffer = Zeroizing::new(vec![0u8; 8192]); let mut filled = 0;
                    while filled < buffer.len() && !buffer[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
                        check()?;
                        stream.set_read_timeout(Some(remaining()?.min(Duration::from_millis(100)))).map_err(CoreError::io)?;
                        match stream.read(&mut buffer[filled..]) {
                            Ok(0) => break,
                            Ok(n) => filled += n,
                            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                            Err(_) => break,
                        }
                    }
                    let request = String::from_utf8_lossy(&buffer[..filled]);
                    let target = Zeroizing::new(request.lines().next().and_then(|l| l.strip_prefix("GET ")).and_then(|l| l.split(' ').next()).unwrap_or("").to_owned());
                    drop(request);
                    let ok = target.starts_with("/callback?") && Url::parse(&format!("http://127.0.0.1{}", target.as_str())).ok()
                        .and_then(|u| u.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| constant_time_eq(v.as_bytes(), pending.state.as_bytes())))
                        .unwrap_or(false);
                    let body = if ok { "Paranoic: вернитесь в приложение. / Return to the app." } else { "Paranoic" };
                    stream.set_write_timeout(Some(remaining()?.min(Duration::from_millis(100)))).map_err(CoreError::io)?;
                    let _ = write!(stream, "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        if ok { "200 OK" } else { "400 Bad Request" }, body.len(), body);
                    if ok { return Ok(format!("http://127.0.0.1:{}{}", self.port, target.as_str())); }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(remaining()?.min(Duration::from_millis(100)));
                }
                Err(error) => return Err(CoreError::io(error)),
            }
        }
    }
}

fn random_token(bytes: usize) -> Result<String> {
    let mut raw = Zeroizing::new(Vec::with_capacity(bytes));
    while raw.len() < bytes {
        let id = paranoic_crypto::ObjectId::generate().map_err(|_| CoreError::new(ErrorCode::Io, "secure random unavailable"))?;
        raw.extend_from_slice(id.as_bytes());
    }
    raw.truncate(bytes);
    Ok(URL_SAFE_NO_PAD.encode(&raw[..]))
}
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }
pub(crate) fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid cloud login response") }
fn locked() -> CoreError { CoreError::new(ErrorCode::PermissionDenied, "cloud session is locked") }
fn poisoned() -> CoreError { CoreError::new(ErrorCode::Internal, "cloud session state unavailable") }
pub(crate) const SESSION_EXPIRED: &str = EXPIRED;

#[cfg(test)]
mod redirect_deadline_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn incomplete_request_obeys_the_overall_deadline() {
        let receiver = LoopbackRedirect::bind().unwrap();
        let mut connection = std::net::TcpStream::connect(("127.0.0.1", receiver.port)).unwrap();
        connection.write_all(b"GET /callback?").unwrap();
        let pending = OAuthClient::onedrive("test", receiver.redirect_uri()).begin().unwrap();
        let started = Instant::now();
        let error = receiver.wait(&pending, Duration::from_millis(150), || Ok(())).unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2), "request blocked past deadline");
    }

    #[test]
    fn incomplete_request_remains_cancellable() {
        let receiver = LoopbackRedirect::bind().unwrap();
        let mut connection = std::net::TcpStream::connect(("127.0.0.1", receiver.port)).unwrap();
        connection.write_all(b"GET /callback?").unwrap();
        let pending = OAuthClient::onedrive("test", receiver.redirect_uri()).begin().unwrap();
        let mut checks = 0;
        let started = Instant::now();
        let error = receiver.wait(&pending, Duration::from_secs(30), || {
            checks += 1;
            if checks >= 3 { Err(CoreError::new(ErrorCode::Cancelled, "cancelled by caller")) } else { Ok(()) }
        }).unwrap_err();
        assert_eq!(error.message, "cancelled by caller");
        assert!(started.elapsed() < Duration::from_secs(2), "request delayed cancellation");
    }
}
