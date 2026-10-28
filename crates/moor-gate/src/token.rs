//! Access token checks against the issuer's published signing keys.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use moor_core::user::{Email, Identity};
use serde::Deserialize;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use crate::NAMESPACE;

/// A token with an unknown key id fetches the key set again at most this often, so forged
/// key ids cannot make the gate hammer the issuer.
const REFETCH_EVERY: Duration = Duration::from_secs(10);

/// Who a verified token speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// `authgear:<sub>`.
    pub identity: Identity,
    /// The token's verified email.
    pub email: Email,
}

/// The claims the gate reads; `exp`, `iss` and `aud` are checked by the validation.
#[derive(Deserialize)]
struct Claims {
    /// The issuer's id for the person.
    sub: String,
    /// The person's email, present when the `email` scope was granted.
    email: Option<String>,
    /// Whether the issuer verified that email.
    email_verified: Option<bool>,
}

/// The key set last fetched, and when.
struct Cache {
    /// The keys.
    keys: JwkSet,
    /// When they were fetched; never when absent.
    fetched: Option<Instant>,
}

/// Checks access tokens from one issuer for one client.
pub struct Verifier {
    /// The expected `iss`.
    issuer: String,
    /// The expected `aud`.
    client_id: String,
    /// Where the keys are; found through the issuer's discovery document when absent.
    jwks_url: Option<String>,
    /// The keys fetched so far.
    cache: Mutex<Cache>,
    /// Fetches the discovery document and the keys.
    agent: Agent,
}

impl Verifier {
    /// A verifier for tokens `issuer` signed for `client_id`.
    #[must_use]
    pub fn new(issuer: String, client_id: String, jwks_url: Option<String>) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(10)))
            .build();
        Self {
            issuer,
            client_id,
            jwks_url,
            cache: Mutex::new(Cache {
                keys: JwkSet { keys: Vec::new() },
                fetched: None,
            }),
            agent: Agent::new_with_config(config),
        }
    }

    /// The expected issuer.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The expected audience.
    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The person `token` speaks for.
    ///
    /// # Errors
    /// Why the token is refused.
    pub fn verify(&self, token: &str) -> Result<Person, String> {
        let header = decode_header(token).map_err(|e| format!("the token is not a JWT: {e}"))?;
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err("a token signed with a shared secret is refused".to_owned());
        }
        let key = self.key(header.kid.as_deref())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|e| format!("the token is refused: {e}"))?
            .claims;
        let email = claims.email.ok_or_else(|| {
            "the token carries no email; sign in again with the email scope".to_owned()
        })?;
        if claims.email_verified != Some(true) {
            return Err(format!("the token's email {email} is not verified"));
        }
        Ok(Person {
            identity: Identity::parse(&format!("{NAMESPACE}:{}", claims.sub))?,
            email: Email::parse(&email)?,
        })
    }

    /// The key `kid` names, fetching the key set again when it is not known yet.
    fn key(&self, kid: Option<&str>) -> Result<DecodingKey, String> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(key) = find(&cache.keys, kid) {
            return key;
        }
        #[allow(
            clippy::disallowed_methods,
            reason = "a refetch rate limit, not a time anything stores or shows"
        )]
        let now = Instant::now();
        if cache.fetched.is_none_or(|at| at.elapsed() >= REFETCH_EVERY) {
            cache.fetched = Some(now);
            cache.keys = self.fetch()?;
        }
        find(&cache.keys, kid)
            .unwrap_or_else(|| Err("no signing key of the issuer matches the token".to_owned()))
    }

    /// The issuer's current key set.
    fn fetch(&self) -> Result<JwkSet, String> {
        let url = if let Some(url) = &self.jwks_url {
            url.clone()
        } else {
            let discovery = format!(
                "{}/.well-known/openid-configuration",
                self.issuer.trim_end_matches('/')
            );
            let doc: serde_json::Value = self.get_json(&discovery)?;
            doc.get("jwks_uri")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("{discovery} names no jwks_uri"))?
                .to_owned()
        };
        self.get_json(&url)
    }

    /// GETs `url` and parses its JSON body.
    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        let body = self
            .agent
            .get(url)
            .call()
            .and_then(|mut res| res.body_mut().read_to_string())
            .map_err(|e| format!("fetching {url} failed: {e}"))?;
        serde_json::from_str(&body).map_err(|e| format!("{url} is not the expected JSON: {e}"))
    }
}

/// The key `kid` names in `keys`, or the only key when the token names none.
fn find(keys: &JwkSet, kid: Option<&str>) -> Option<Result<DecodingKey, String>> {
    let jwk = match kid {
        Some(kid) => keys.find(kid)?,
        None => match keys.keys.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    Some(DecodingKey::from_jwk(jwk).map_err(|e| format!("the issuer's key is unusable: {e}")))
}
