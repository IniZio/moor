//! The HTTP side of sign-in: discovery documents and the token endpoint.

use std::sync::Arc;
use std::time::Duration;

use oauth2::url::Url;
use serde::de::DeserializeOwned;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{http, Agent};

/// The most a discovery or token response may hold.
const BODY_LIMIT: u64 = 1 << 20;

/// An HTTP client that hands back every status; only a download client follows redirects.
pub struct Http(Agent);

impl Http {
    /// A client trusting the platform's roots.
    #[must_use]
    pub fn new() -> Self {
        Self::build(Duration::from_secs(15), 0)
    }

    /// A client for downloads that gives up after `timeout` and follows redirects, over
    /// `https` only: release hosts answer from a storage host, and the caller checks what comes.
    #[must_use]
    pub fn downloads(timeout: Duration) -> Self {
        Self::build(timeout, 5)
    }

    /// A client trusting the platform's roots.
    fn build(timeout: Duration, redirects: u32) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .max_redirects(redirects)
            .https_only(redirects > 0)
            .build();
        Self(Agent::new_with_config(config))
    }

    /// The JSON document at `url`.
    ///
    /// # Errors
    /// The address is not `https` (or loopback `http`), cannot be reached, answers an error
    /// status, or answers something unreadable.
    pub fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        secure(url)?;
        let mut resp = self
            .0
            .get(url)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| format!("cannot reach {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{url} answered {}", resp.status()));
        }
        let bytes = read(resp.body_mut()).map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
    }

    /// The body at `url`, at most `limit` bytes.
    ///
    /// # Errors
    /// As [`Http::get_json`]; a body over `limit` is an error too.
    pub fn get_bytes(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        secure(url)?;
        let mut resp = self
            .0
            .get(url)
            .call()
            .map_err(|e| format!("cannot reach {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{url} answered {}", resp.status()));
        }
        resp.body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| format!("{url}: {e}"))
    }

    /// Posts `body` as JSON to `url` and reads the JSON answer.
    ///
    /// # Errors
    /// As [`Http::get_json`]; an error status carries the answer's `error` field.
    pub fn post_json<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<T, String> {
        secure(url)?;
        let mut resp = self
            .0
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body.to_string())
            .map_err(|e| format!("cannot reach {url}: {e}"))?;
        let status = resp.status();
        let bytes = read(resp.body_mut()).map_err(|e| format!("{url}: {e}"))?;
        if !status.is_success() {
            let why = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(format!("{url} answered {status}: {why}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// Refuses an address that is not `https`; `http` is for a server on this machine.
fn secure(address: &str) -> Result<(), String> {
    let url = Url::parse(address).map_err(|e| format!("`{address}`: {e}"))?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() == "https" || (url.scheme() == "http" && local) {
        Ok(())
    } else {
        Err(format!("{address} is not https"))
    }
}

/// A response body up to [`BODY_LIMIT`].
fn read(body: &mut ureq::Body) -> Result<Vec<u8>, ureq::Error> {
    body.with_config().limit(BODY_LIMIT).read_to_vec()
}

impl oauth2::SyncHttpClient for Http {
    type Error = ureq::Error;

    fn call(&self, request: oauth2::HttpRequest) -> Result<oauth2::HttpResponse, Self::Error> {
        let (parts, mut body) = self.0.run(request)?.into_parts();
        Ok(http::Response::from_parts(parts, read(&mut body)?))
    }
}
