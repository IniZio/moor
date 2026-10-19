//! Token authentication and CSRF checks for the web front.

use std::fs::File;
use std::io::{self, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use sha2::{Digest as _, Sha256};

/// Name of the cookie that carries the token.
pub const COOKIE: &str = "moor_token";

/// How a request proved it holds the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// `Authorization: Bearer` header.
    Header,
    /// The token cookie.
    Cookie,
    /// The `?token=` query parameter.
    Query,
}

/// The credential-bearing parts of a request.
#[derive(Debug, Clone, Copy)]
pub struct Credentials<'a> {
    /// HTTP method.
    pub method: &'a str,
    /// Request path without the query string.
    pub path: &'a str,
    /// Token after `Bearer ` in the `Authorization` header.
    pub bearer: Option<&'a str>,
    /// Value of the token cookie.
    pub cookie: Option<&'a str>,
    /// Value of the `token` query parameter.
    pub query_token: Option<&'a str>,
}

/// Checks the request's credentials; the query token counts only on `GET /`.
#[must_use]
pub fn authenticate(creds: &Credentials<'_>, token: &str) -> Option<Via> {
    if creds.bearer.is_some_and(|t| token_matches(token, t)) {
        return Some(Via::Header);
    }
    if creds.cookie.is_some_and(|t| token_matches(token, t)) {
        return Some(Via::Cookie);
    }
    let on_page = creds.method == "GET" && creds.path == "/";
    creds
        .query_token
        .filter(|_| on_page)
        .is_some_and(|t| token_matches(token, t))
        .then_some(Via::Query)
}

/// Compares two tokens without an early exit on the first differing byte.
#[must_use]
pub fn token_matches(expected: &str, given: &str) -> bool {
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.as_bytes());
    let diff = a
        .iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    !expected.is_empty() && diff == 0
}

/// The headers a state-changing request is checked on.
#[derive(Debug, Clone, Copy)]
pub struct CsrfHeaders<'a> {
    /// `X-Moor-Csrf`.
    pub csrf: Option<&'a str>,
    /// `Content-Type`.
    pub content_type: Option<&'a str>,
    /// `Host`.
    pub host: Option<&'a str>,
    /// `Origin`.
    pub origin: Option<&'a str>,
    /// `Sec-Fetch-Site`.
    pub sec_fetch_site: Option<&'a str>,
}

/// Rejects requests a browser page on another origin could have forged.
///
/// A browser cannot add a custom header to a cross-origin request without a CORS preflight,
/// and the server never answers one, so the header alone defeats forged form posts and fetches.
/// The Host, Origin and Sec-Fetch-Site checks are defence in depth.
///
/// The UI is reached through a port forward, so the browser's port can differ from the bound
/// one: the port is not compared. Host must name loopback, which keeps DNS-rebinding pages
/// (whose Host is their own domain) out, and Origin, when present, must match Host exactly.
pub fn check_csrf(headers: &CsrfHeaders<'_>) -> Result<(), &'static str> {
    if headers.csrf != Some("1") {
        return Err("missing X-Moor-Csrf header");
    }
    let json = headers.content_type.is_some_and(|v| {
        v.trim()
            .to_ascii_lowercase()
            .starts_with("application/json")
    });
    if !json {
        return Err("content type must be application/json");
    }
    let Some(host) = headers.host else {
        return Err("missing Host header");
    };
    if !is_loopback_host(host) {
        return Err("bad host");
    }
    if headers
        .origin
        .is_some_and(|o| o.strip_prefix("http://") != Some(host))
    {
        return Err("bad origin");
    }
    if headers
        .sec_fetch_site
        .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return Err("cross-site request");
    }
    Ok(())
}

/// Whether a `Host` header value names loopback, with any port.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix("[::1]") {
        Some(rest) => return rest.is_empty() || rest.starts_with(':'),
        None => host.split(':').next(),
    };
    matches!(name, Some("127.0.0.1" | "localhost"))
}

/// Reads the token from `path`, which must be owner-only and non-empty.
pub fn read_token(path: &Path) -> Result<String, String> {
    let shown = path.display();
    let fail = |why: &dyn std::fmt::Display| format!("web token file {shown}: {why}");
    let read = || -> io::Result<Result<String, &'static str>> {
        let mut file = File::open(path)?;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Ok(Err("must be mode 0600"));
        }
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let token = text.trim_end();
        Ok(if token.is_empty() {
            Err("is empty")
        } else {
            Ok(token.to_owned())
        })
    };
    match read() {
        Ok(Ok(token)) => Ok(token),
        Ok(Err(why)) => Err(fail(&why)),
        Err(e) => Err(fail(&e)),
    }
}

/// The `Set-Cookie` value that stores `token` for the browser.
#[must_use]
pub fn cookie_header(token: &str) -> String {
    format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/")
}

#[cfg(test)]
mod tests;
