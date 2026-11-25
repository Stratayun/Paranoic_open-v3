//! Shared blocking HTTPS client: rustls, no automatic redirects, bounded bodies.
use std::time::Duration;
use reqwest::{blocking::{Client, RequestBuilder, Response}, StatusCode};
use crate::{CoreError, ErrorCode, Result};

#[derive(Clone)]
pub struct Http { client: Client }

impl Http {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .user_agent("Paranoic")
            .build()
            .map_err(|_| CoreError::new(ErrorCode::Io, "HTTPS client unavailable"))?;
        Ok(Self { client })
    }
    /// Client that also permits plain http — ONLY for a loopback WebDAV server
    /// (WebDav::new refuses http for any other host).
    pub fn new_loopback_http() -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .user_agent("Paranoic")
            .build()
            .map_err(|_| CoreError::new(ErrorCode::Io, "HTTP client unavailable"))?;
        Ok(Self { client })
    }
    pub(crate) fn client(&self) -> &Client { &self.client }

    /// Send a request. Transport failures of non-idempotent requests are
    /// uncertain (the server may have applied them) and never retried here.
    pub(crate) fn send(&self, request: RequestBuilder, idempotent: bool) -> Result<Response> {
        let response = request.send().map_err(|error| if idempotent || error.is_connect() {
            CoreError::new(ErrorCode::Io, "cloud request failed") } else { crate::uncertain() })?;
        Ok(response)
    }
    /// Send and require 2xx; error statuses are mapped and the body discarded.
    pub(crate) fn ok(&self, request: RequestBuilder, idempotent: bool) -> Result<Response> {
        let response = self.send(request, idempotent)?;
        if response.status().is_success() { Ok(response) } else { Err(status_error(&response, idempotent)) }
    }
}



pub(crate) fn status_error(response: &Response, idempotent: bool) -> CoreError {
    let status = response.status();
    match status {
        StatusCode::UNAUTHORIZED => CoreError::new(ErrorCode::PermissionDenied, SESSION_EXPIRED),
        StatusCode::FORBIDDEN => CoreError::new(ErrorCode::PermissionDenied, "cloud provider denied the request"),
        StatusCode::NOT_FOUND | StatusCode::GONE => CoreError::new(ErrorCode::NotFound, "cloud object not found"),
        StatusCode::CONFLICT => crate::exists(),
        StatusCode::PRECONDITION_FAILED => crate::stale(),
        StatusCode::INSUFFICIENT_STORAGE | StatusCode::PAYLOAD_TOO_LARGE => CoreError::new(ErrorCode::LimitExceeded, "cloud storage quota or size limit"),
        StatusCode::TOO_MANY_REQUESTS => CoreError::new(ErrorCode::Busy, "cloud provider rate limit; retry later"),
        s if s.is_server_error() && !idempotent => crate::uncertain(),
        s if s.is_server_error() => CoreError::new(ErrorCode::Busy, "cloud provider temporarily unavailable"),
        _ => CoreError::new(ErrorCode::Io, "cloud provider rejected the request"),
    }
}


/// The server refused the credentials (expired or revoked).
pub(crate) const SESSION_EXPIRED: &str = "cloud session expired; sign in again";
