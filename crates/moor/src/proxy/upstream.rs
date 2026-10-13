//! Per-route upstream base URLs and the `Authorization` value injected into
//! forwarded requests.

use std::collections::BTreeMap;

use moor_core::proxy_gate::{Inject, RouteId};
use moor_core::Secret;

use crate::env::Env;

/// Route ids paired with their production upstream base.
const DEFAULTS: [(&str, &str); 3] = [
    ("model", "https://api.anthropic.com"),
    ("forge.git", "https://github.com"),
    ("forge.api", "https://api.github.com"),
];

/// A credential could not be produced for a route.
///
/// The text is fixed so no secret can reach a log through it.
#[derive(Debug, thiserror::Error)]
#[error("credential unavailable")]
pub(crate) struct CredError;

/// Supplies the credential injected for a route.
pub(crate) trait RouteCredential: Send + Sync {
    /// The secret to inject for `route`.
    fn credential(&self, route: &RouteId) -> Result<Secret, CredError>;
}

/// Base URL per route id, without a trailing `/`.
#[derive(Debug, Clone)]
pub(crate) struct Upstreams {
    /// Route id to base URL.
    bases: BTreeMap<String, String>,
}

/// An upstream base was rejected.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpstreamError {
    /// The base uses a scheme that is not allowed.
    #[error("upstream for route {route} must use https")]
    InsecureScheme {
        /// Route whose base was rejected.
        route: String,
    },
    /// The base is not an absolute URL with a host.
    #[error("upstream for route {route} is not a valid URL")]
    BadUrl {
        /// Route whose base was rejected.
        route: String,
    },
}

impl Upstreams {
    /// Production defaults, with `MOOR_PROXY_UPSTREAM_<ROUTE>` overrides applied
    /// only when `MOOR_TEST=1`; the overrides are ignored otherwise.
    pub(crate) fn from_env(env: &Env) -> Result<Self, UpstreamError> {
        let test = env.test_mode();
        let bases = DEFAULTS.iter().map(|(route, default)| {
            let over = if test {
                env.proxy_upstream(route)
            } else {
                None
            };
            ((*route).to_owned(), over.unwrap_or(default).to_owned())
        });
        Self::new(bases, test)
    }

    /// Validates and stores the bases. Every base must be `https://`;
    /// `http://` is accepted only when `allow_http` is true.
    pub(crate) fn new(
        bases: impl IntoIterator<Item = (String, String)>,
        allow_http: bool,
    ) -> Result<Self, UpstreamError> {
        let mut out = BTreeMap::new();
        for (route, base) in bases {
            let base = check_base(&route, &base, allow_http)?;
            out.insert(route, base);
        }
        Ok(Self { bases: out })
    }

    /// Base URL of `route`.
    pub(crate) fn base(&self, route: &RouteId) -> Option<&str> {
        self.bases.get(route.as_str()).map(String::as_str)
    }

    /// Base plus `upstream_path`, which starts with `/` and carries any query.
    pub(crate) fn url(&self, route: &RouteId, upstream_path: &str) -> Option<String> {
        self.base(route).map(|b| format!("{b}{upstream_path}"))
    }
}

/// Validates one base and returns it without trailing slashes.
fn check_base(route: &str, base: &str, allow_http: bool) -> Result<String, UpstreamError> {
    let bad = || UpstreamError::BadUrl {
        route: route.to_owned(),
    };
    let (scheme, rest) = base.split_once("://").ok_or_else(bad)?;
    let secure = scheme.eq_ignore_ascii_case("https");
    if !(secure || (allow_http && scheme.eq_ignore_ascii_case("http"))) {
        return Err(UpstreamError::InsecureScheme {
            route: route.to_owned(),
        });
    }
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.chars().any(char::is_whitespace) {
        return Err(bad());
    }
    Ok(format!("{scheme}://{trimmed}"))
}

/// `Authorization` header value for `inject`, or `None` for [`Inject::None`].
///
/// Returned as a [`Secret`] so the value has no `Debug` or `Display` path.
pub(crate) fn auth_value(inject: Inject, secret: &Secret) -> Option<Secret> {
    let mut out = Vec::new();
    match inject {
        Inject::None => return None,
        Inject::Bearer => {
            out.extend_from_slice(b"Bearer ");
            out.extend_from_slice(secret.expose());
        }
        Inject::Basic { user } => {
            let mut raw = user.as_bytes().to_vec();
            raw.push(b':');
            raw.extend_from_slice(secret.expose());
            out.extend_from_slice(b"Basic ");
            out.extend_from_slice(base64(&raw).as_bytes());
        }
    }
    Some(Secret::new(out))
}

/// Standard-alphabet base64 with `=` padding.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sym = |n: u32| char::from(ALPHABET.get((n & 63) as usize).copied().unwrap_or(b'='));
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let mut it = chunk.iter().map(|&b| u32::from(b));
        let (a, b, c) = (it.next().unwrap_or(0), it.next(), it.next());
        let n = (a << 16) | (b.unwrap_or(0) << 8) | c.unwrap_or(0);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if b.is_some() { sym(n >> 6) } else { '=' });
        out.push(if c.is_some() { sym(n) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests;
