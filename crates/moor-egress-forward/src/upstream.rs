//! The shared Proxy a forwarder dials.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::{ClientConfig, RootCertStore};
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::PATIENCE;

/// The shared Proxy's `/egress` address and the token presented there.
pub struct Upstream {
    /// `ws://` or `wss://` address.
    pub(crate) url: String,
    /// `Bearer <token>`; none passes on each client's proxy password.
    pub(crate) bearer: Option<HeaderValue>,
    /// TLS for a `wss://` address.
    pub(crate) tls: Arc<ClientConfig>,
    /// How long a CONNECT keeps dialing while the Proxy cannot be reached.
    pub(crate) patience: Duration,
}

impl fmt::Debug for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upstream")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Upstream {
    /// `url` with `token`, trusting `roots` for `wss://`.
    ///
    /// # Errors
    /// As for [`crate::Config::parse`].
    pub fn new(url: String, token: &str, roots: RootCertStore) -> Result<Self, String> {
        if token.is_empty() {
            return Err("the token is empty".to_owned());
        }
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "the token cannot be sent in a header".to_owned())?;
        bearer.set_sensitive(true);
        Self::with_bearer(url, Some(bearer), roots)
    }

    /// `url`, passing on each client's proxy password, trusting `roots` for `wss://`.
    ///
    /// # Errors
    /// The upstream is not a `ws://` or `wss://` address.
    pub fn passing_leases(url: String, roots: RootCertStore) -> Result<Self, String> {
        Self::with_bearer(url, None, roots)
    }

    /// `url` with `bearer`, trusting `roots` for `wss://`.
    fn with_bearer(
        url: String,
        bearer: Option<HeaderValue>,
        roots: RootCertStore,
    ) -> Result<Self, String> {
        match (url.strip_prefix("ws://"), url.starts_with("wss://")) {
            (Some(rest), _) if !loopback(rest) => {
                return Err(format!(
                    "upstream {url} sends the token in the clear; only a loopback ws:// address may"
                ))
            }
            (Some(_), _) | (None, true) => {}
            (None, false) => {
                return Err(format!("upstream {url} is not a ws:// or wss:// address"))
            }
        }
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            url,
            bearer,
            tls: Arc::new(tls),
            patience: PATIENCE,
        })
    }

    /// This upstream, with a CONNECT dialing for at most `patience` while it cannot be reached.
    #[must_use]
    pub const fn with_patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }
}

/// Whether the host of `rest`, an address after its scheme, is a loopback name or address.
fn loopback(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or_default();
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or_default(),
        |v6| v6.split(']').next().unwrap_or_default(),
    );
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}
