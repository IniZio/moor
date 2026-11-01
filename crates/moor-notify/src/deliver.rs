//! Posting one delivery to a webhook's URL, with retries and backoff.

use std::sync::Arc;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::transport::DefaultConnector;
use ureq::Agent;

use crate::guard::{NotPublic, PublicOnly, Reach, NOT_PUBLIC};

use crate::sign::signature;

/// Time limit of one POST.
const POST_TIMEOUT: Duration = Duration::from_secs(15);

/// How often and how far apart one event is posted before it is given up.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Most attempts, the first included.
    pub attempts: u32,
    /// Wait after the first failure; it doubles after each further one.
    pub first: Duration,
    /// Longest wait between attempts.
    pub max: Duration,
}

impl Default for Retry {
    /// Eight attempts over about four minutes.
    fn default() -> Self {
        Self {
            attempts: 8,
            first: Duration::from_secs(2),
            max: Duration::from_secs(60),
        }
    }
}

/// How the receiver answered one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// A 2xx status: delivered.
    Taken,
    /// No answer, a timeout, 408, 429 or 5xx: worth another attempt.
    Retry(String),
    /// Any other status, or an address that is not public: another attempt would get the
    /// same, for this reason.
    Refused(String),
}

impl Answer {
    /// The answer an HTTP `status` means.
    #[must_use]
    pub fn of_status(status: u16) -> Self {
        match status {
            200..=299 => Self::Taken,
            408 | 429 | 500..=599 => Self::Retry(format!("HTTP {status}")),
            _ => Self::Refused(format!("HTTP {status}, not retried")),
        }
    }
}

/// Posts with `post` until it is taken, refused, or `retry` runs out, sleeping with `sleep`
/// between attempts.
///
/// # Errors
/// Why the last attempt failed, and after how many.
pub fn with_retries(
    retry: Retry,
    mut post: impl FnMut() -> Answer,
    sleep: &dyn Fn(Duration),
) -> Result<u32, String> {
    let mut wait = retry.first;
    for attempt in 1..=retry.attempts.max(1) {
        match post() {
            Answer::Taken => return Ok(attempt),
            Answer::Refused(why) => return Err(why),
            Answer::Retry(why) if attempt >= retry.attempts => {
                return Err(format!("{why} after {attempt} attempts"));
            }
            Answer::Retry(_) => {
                sleep(wait);
                wait = wait.saturating_mul(2).min(retry.max);
            }
        }
    }
    Err("no attempt was made".to_owned())
}

/// A short reason for a failed attempt that never names the URL, which may hold a token.
const fn describe(error: &ureq::Error) -> &'static str {
    match error {
        ureq::Error::Timeout(_) => "timed out",
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
            "connection failed"
        }
        _ => "request failed",
    }
}

/// One hook's endpoint: where to post, the key that signs each body, and any header of the
/// receiver's own.
pub struct Endpoint {
    /// The HTTP client.
    agent: Agent,
    /// The URL posted to.
    url: String,
    /// The HMAC key.
    secret: Vec<u8>,
    /// The receiver's own header, as name and value.
    header: Option<(String, Vec<u8>)>,
}

impl Endpoint {
    /// An endpoint for `url` signed with `secret` that connects only to addresses `reach`
    /// allows. It goes through the proxy the environment names, if any.
    #[must_use]
    pub fn new(url: String, secret: Vec<u8>, reach: Reach) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(POST_TIMEOUT))
            .build();
        let agent = match reach {
            Reach::Public => {
                Agent::with_parts(config, DefaultConnector::default(), PublicOnly::default())
            }
            Reach::Any => Agent::new_with_config(config),
        };
        Self {
            agent,
            url,
            secret,
            header: None,
        }
    }

    /// The endpoint, also sending `name: value` on every post.
    #[must_use]
    pub fn with_header(self, name: String, value: Vec<u8>) -> Self {
        Self {
            header: Some((name, value)),
            ..self
        }
    }

    /// Posts `body` once, as event `kind` with delivery id `delivery`, signed at `now`.
    #[must_use]
    pub fn post(&self, kind: &str, delivery: i64, body: &[u8], now: u64) -> Answer {
        let mut request = self.agent.post(&self.url);
        if let Some((name, value)) = &self.header {
            request = request.header(name.as_str(), value.as_slice());
        }
        let sent = request
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                concat!("moor-notify/", env!("CARGO_PKG_VERSION")),
            )
            .header("X-Moor-Event", kind)
            .header("X-Moor-Delivery", delivery.to_string())
            .header("X-Moor-Timestamp", now.to_string())
            .header("X-Moor-Signature", signature(&self.secret, now, body))
            .send(body);
        match sent {
            Ok(response) => Answer::of_status(response.status().as_u16()),
            Err(ureq::Error::Other(e)) if e.is::<NotPublic>() => {
                Answer::Refused(NOT_PUBLIC.to_owned())
            }
            Err(e) => Answer::Retry(describe(&e).to_owned()),
        }
    }

    /// The URL's scheme, host and port, for a log line that must not carry a token from its
    /// path, query or user part.
    #[must_use]
    pub fn shown(&self) -> String {
        self.url.parse::<ureq::http::Uri>().map_or_else(
            |_| "the hook URL".to_owned(),
            |uri| {
                let port = uri.port_u16().map(|p| format!(":{p}")).unwrap_or_default();
                format!(
                    "{}://{}{port}",
                    uri.scheme_str().unwrap_or_default(),
                    uri.host().unwrap_or_default()
                )
            },
        )
    }
}

#[cfg(test)]
mod tests;
