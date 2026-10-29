//! Access token checks against the issuer's published signing keys.

use std::collections::HashMap;
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

/// How long a userinfo email answers for its `sub`. A front WebSocket verifies once, but every
/// `/mcp` request verifies its token; five minutes keeps that to one userinfo call per person
/// per five minutes while an email the issuer unverifies stops working within that time.
const EMAIL_TTL: Duration = Duration::from_secs(300);

/// Who a verified token speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// `authgear:<sub>`.
    pub identity: Identity,
    /// The token's verified email.
    pub email: Email,
}

/// One audience or several, as `aud` may be either.
#[derive(Deserialize)]
#[serde(untagged)]
enum Aud {
    /// A single audience.
    One(String),
    /// Several audiences.
    Many(Vec<String>),
}

impl Aud {
    /// Whether `want` is among the audiences.
    fn names(&self, want: &str) -> bool {
        match self {
            Self::One(aud) => aud == want,
            Self::Many(auds) => auds.iter().any(|aud| aud == want),
        }
    }
}

/// Who a token must be for.
#[derive(Clone, Copy)]
enum For<'a> {
    /// The gate's own client: `aud` names it, or `client_id` is it.
    Client,
    /// An API resource: `aud` names it.
    Resource(&'a str),
    /// An OIDC id token for the gate's own client: `aud` names it and the token itself
    /// carries the verified email.
    IdToken,
}

/// The claims the gate reads; `exp` and `iss` are checked by the validation.
#[derive(Deserialize)]
struct Claims {
    /// The issuer's id for the person.
    sub: String,
    /// Who the token is for.
    aud: Aud,
    /// The client the token was issued to.
    client_id: Option<String>,
    /// The person's email, present when the `email` scope was granted.
    email: Option<String>,
    /// Whether the issuer verified that email.
    email_verified: Option<bool>,
    /// The scopes granted, space separated.
    scope: Option<String>,
}

/// What the issuer's userinfo endpoint says about the person.
#[derive(Deserialize)]
struct Userinfo {
    /// Must be the token's `sub`.
    sub: String,
    /// The person's email.
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
    /// The issuer's userinfo endpoint, once its discovery document was read.
    userinfo_url: Mutex<Option<String>>,
    /// Verified emails from the userinfo endpoint by `sub`, and when each was fetched.
    emails: Mutex<HashMap<String, (Email, Instant)>>,
    /// Fetches the discovery document, the keys and userinfo.
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
            userinfo_url: Mutex::new(None),
            emails: Mutex::new(HashMap::new()),
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
        self.check(token, For::Client).map(|(person, _)| person)
    }

    /// The person `token` speaks for and the scopes it grants, when it was issued for the API
    /// resource `resource` rather than for the gate's own client.
    ///
    /// # Errors
    /// Why the token is refused.
    pub fn verify_resource(
        &self,
        token: &str,
        resource: &str,
    ) -> Result<(Person, Vec<String>), String> {
        self.check(token, For::Resource(resource))
    }

    /// The person an id token the gate's own client was issued speaks for.
    ///
    /// # Errors
    /// Why the token is refused.
    pub fn verify_id_token(&self, token: &str) -> Result<Person, String> {
        self.check(token, For::IdToken).map(|(person, _)| person)
    }

    /// Checks `token` is for `audience`.
    fn check(&self, token: &str, audience: For<'_>) -> Result<(Person, Vec<String>), String> {
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
        validation.validate_aud = false;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|e| format!("the token is refused: {e}"))?
            .claims;
        let for_us = match audience {
            For::Client => {
                claims.aud.names(&self.client_id)
                    || claims.client_id.as_deref() == Some(self.client_id.as_str())
            }
            For::Resource(resource) => claims.aud.names(resource),
            For::IdToken => claims.aud.names(&self.client_id),
        };
        if !for_us {
            return Err("the token is refused: it is for another audience".to_owned());
        }
        let email = match (claims.email, claims.email_verified) {
            (Some(email), Some(true)) => Email::parse(&email)?,
            (Some(email), Some(false)) => {
                return Err(format!("the token's email {email} is not verified"))
            }
            _ if matches!(audience, For::IdToken) => {
                return Err("the id token carries no verified email".to_owned())
            }
            _ => self.userinfo_email(token, &claims.sub)?,
        };
        let scopes = claims
            .scope
            .iter()
            .flat_map(|s| s.split_whitespace().map(str::to_owned))
            .collect();
        let person = Person {
            identity: Identity::parse(&format!("{NAMESPACE}:{}", claims.sub))?,
            email,
        };
        Ok((person, scopes))
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

    /// The verified email the issuer's userinfo endpoint gives for `sub`, asked with `token`.
    fn userinfo_email(&self, token: &str, sub: &str) -> Result<Email, String> {
        #[allow(
            clippy::disallowed_methods,
            reason = "a cache expiry, not a time anything stores or shows"
        )]
        let now = Instant::now();
        let mut emails = self.emails.lock().unwrap_or_else(PoisonError::into_inner);
        emails.retain(|_, (_, at)| now.duration_since(*at) < EMAIL_TTL);
        if let Some((email, _)) = emails.get(sub) {
            return Ok(email.clone());
        }
        drop(emails);
        let url = self.userinfo_url()?;
        let info: Userinfo = self
            .get_json(&url, Some(token))
            .map_err(|e| format!("the token carries no email and asking userinfo failed: {e}"))?;
        if info.sub != sub {
            return Err(format!("{url} answered for another subject"));
        }
        let email = info
            .email
            .ok_or_else(|| format!("{url} gives no email; sign in again with the email scope"))?;
        if info.email_verified != Some(true) {
            return Err(format!("the email {email} is not verified"));
        }
        let email = Email::parse(&email)?;
        self.emails
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(sub.to_owned(), (email.clone(), now));
        Ok(email)
    }

    /// The issuer's userinfo endpoint, reading the discovery document when not known yet.
    fn userinfo_url(&self) -> Result<String, String> {
        if let Some(url) = &*self
            .userinfo_url
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return Ok(url.clone());
        }
        self.discover("userinfo_endpoint")
    }

    /// The URL `field` of the issuer's discovery document, remembering its userinfo endpoint.
    fn discover(&self, field: &str) -> Result<String, String> {
        let discovery = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        let doc: serde_json::Value = self.get_json(&discovery, None)?;
        let url = |name: &str| {
            doc.get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        if let Some(userinfo) = url("userinfo_endpoint") {
            *self
                .userinfo_url
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(userinfo);
        }
        url(field).ok_or_else(|| format!("{discovery} names no {field}"))
    }

    /// The issuer's current key set.
    fn fetch(&self) -> Result<JwkSet, String> {
        let url = match &self.jwks_url {
            Some(url) => url.clone(),
            None => self.discover("jwks_uri")?,
        };
        self.get_json(&url, None)
    }

    /// GETs `url`, with `bearer` as its access token when given, and parses its JSON body.
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<T, String> {
        let mut req = self.agent.get(url);
        if let Some(token) = bearer {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let body = req
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
