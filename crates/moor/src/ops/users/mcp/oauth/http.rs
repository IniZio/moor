//! The HTTP side of an MCP sign-in: the probe, discovery and client registration.

use std::sync::Arc;
use std::time::Duration;

use oauth2::url::Url;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use ureq::http;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

/// The most a discovery, registration or token response may hold.
const BODY_LIMIT: u64 = 1 << 20;

/// The `initialize` an unauthenticated MCP server answers with 401.
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"moor","version":"1"}}}"#;

/// An HTTP client that follows no redirect and hands back every status.
pub(super) struct Http(Agent);

/// Protected resource metadata (RFC 9728).
#[derive(Deserialize)]
pub(super) struct Protected {
    /// The resource the metadata describes.
    resource: String,
    /// Issuers that grant tokens for it.
    pub(super) authorization_servers: Vec<String>,
    /// Scopes it understands.
    #[serde(default)]
    pub(super) scopes_supported: Vec<String>,
}

/// Authorization server metadata (RFC 8414).
#[derive(Deserialize)]
pub(super) struct Server {
    /// The issuer, equal to the one discovery started from.
    pub(super) issuer: String,
    /// Where the browser signs in.
    pub(super) authorization_endpoint: String,
    /// Where codes become tokens.
    pub(super) token_endpoint: String,
    /// Where clients register themselves, when the server allows it.
    pub(super) registration_endpoint: Option<String>,
    /// The PKCE methods it accepts.
    #[serde(default)]
    pub(super) code_challenge_methods_supported: Vec<String>,
    /// Whether it names itself in `iss` on the redirect (RFC 9207).
    #[serde(default)]
    pub(super) authorization_response_iss_parameter_supported: bool,
}

/// A client registration's answer.
#[derive(Deserialize)]
pub(super) struct Registered {
    /// The issued client id.
    pub(super) client_id: String,
    /// The issued secret, for a confidential client.
    #[serde(default)]
    pub(super) client_secret: Option<String>,
}

impl Http {
    /// A client trusting the platform's roots.
    pub(super) fn new() -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self(Agent::new_with_config(config))
    }

    /// The resource metadata address of the MCP server at `url`, when an unauthenticated
    /// `initialize` gets a 401 naming one; `None` for any other answer or none at all.
    pub(super) fn probe(&self, url: &str) -> Option<Url> {
        let resp = self
            .0
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .send(INITIALIZE)
            .ok()?;
        if resp.status() != http::StatusCode::UNAUTHORIZED {
            return None;
        }
        let challenge = resp.headers().get("www-authenticate")?.to_str().ok()?;
        Url::parse(auth_param(challenge, "resource_metadata")?).ok()
    }

    /// The protected resource metadata at `at`, which must describe `server`.
    ///
    /// # Errors
    /// It cannot be fetched or read, or describes another resource.
    pub(super) fn resource(&self, at: &Url, server: &str) -> Result<Protected, String> {
        let meta: Protected = self
            .get_json(at)?
            .ok_or_else(|| format!("no resource metadata at {at}"))?;
        if meta.resource.trim_end_matches('/') != server.trim_end_matches('/') {
            return Err(format!(
                "the resource metadata at {at} describes {}, not {server}",
                meta.resource
            ));
        }
        Ok(meta)
    }

    /// The metadata of the authorization server `issuer`, from the RFC 8414 address first and
    /// then the `OpenID` Connect ones.
    ///
    /// # Errors
    /// None of the well-known addresses answers, or the answer names another issuer.
    pub(super) fn server(&self, issuer: &str) -> Result<Server, String> {
        secure(issuer)?;
        let base = Url::parse(issuer).map_err(|e| format!("issuer `{issuer}`: {e}"))?;
        let path = base.path().trim_end_matches('/');
        let mut tried = Vec::new();
        for at in [
            format!("/.well-known/oauth-authorization-server{path}"),
            format!("/.well-known/openid-configuration{path}"),
            format!("{path}/.well-known/openid-configuration"),
        ] {
            let mut url = base.clone();
            url.set_path(&at);
            url.set_query(None);
            if let Some(meta) = self.get_json::<Server>(&url)? {
                if meta.issuer != issuer {
                    return Err(format!("{url} names issuer {}, not {issuer}", meta.issuer));
                }
                secure(&meta.authorization_endpoint)?;
                secure(&meta.token_endpoint)?;
                if let Some(r) = &meta.registration_endpoint {
                    secure(r)?;
                }
                return Ok(meta);
            }
            tried.push(url.to_string());
        }
        Err(format!(
            "no authorization server metadata at {}",
            tried.join(", ")
        ))
    }

    /// Registers moor as a public client redirecting to `redirect` (RFC 7591).
    ///
    /// # Errors
    /// The registration is refused or its answer cannot be read.
    pub(super) fn register(&self, endpoint: &str, redirect: &str) -> Result<Registered, String> {
        let body = serde_json::json!({
            "client_name": "moor",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let mut resp = self
            .0
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body.to_string())
            .map_err(|e| format!("cannot reach {endpoint}: {e}"))?;
        let status = resp.status();
        let bytes = read(resp.body_mut()).map_err(|e| format!("{endpoint}: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "{endpoint} refused the registration: {status} {}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("{endpoint}: {e}"))
    }

    /// The JSON at `url`, or `None` when it answers with anything but success.
    fn get_json<T: DeserializeOwned>(&self, url: &Url) -> Result<Option<T>, String> {
        let mut resp = self
            .0
            .get(url.as_str())
            .header("Accept", "application/json")
            .call()
            .map_err(|e| format!("cannot reach {url}: {e}"))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let bytes = read(resp.body_mut()).map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{url}: {e}"))
    }
}

impl oauth2::SyncHttpClient for Http {
    type Error = ureq::Error;

    fn call(&self, request: oauth2::HttpRequest) -> Result<oauth2::HttpResponse, Self::Error> {
        let (parts, mut body) = self.0.run(request)?.into_parts();
        Ok(http::Response::from_parts(parts, read(&mut body)?))
    }
}

/// Refuses an authorization server address that is not `https`; tests' fakes serve plain
/// `http` on loopback.
pub(super) fn secure(address: &str) -> Result<(), String> {
    let url = Url::parse(address).map_err(|e| format!("`{address}`: {e}"))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if url.scheme() == "https" || (cfg!(test) && url.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(format!("{address} is not https"))
    }
}

/// A response body up to [`BODY_LIMIT`].
fn read(body: &mut ureq::Body) -> Result<Vec<u8>, ureq::Error> {
    body.with_config().limit(BODY_LIMIT).read_to_vec()
}

/// The value of auth-param `name` in a `WWW-Authenticate` challenge, quoted or not.
fn auth_param<'a>(challenge: &'a str, name: &str) -> Option<&'a str> {
    let (_, rest) = challenge.split_once(&format!("{name}="))?;
    rest.strip_prefix('"').map_or_else(
        || rest.split([',', ' ']).next(),
        |quoted| quoted.split('"').next(),
    )
}
