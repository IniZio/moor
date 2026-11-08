//! Browser sign-in: the authorization-code flow with PKCE against the issuer, and short
//! sessions, for pages that must only reach a signed-in person. A started sign-in and a
//! session both live in sealed cookies, so any gate replica sharing the key can serve them.

pub(crate) mod pair;
pub(crate) mod routes;
mod seal;
pub(crate) mod secrets;
pub(crate) mod setup;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use oauth2::basic::BasicClient;
use oauth2::url::Url;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use serde::{Deserialize, Serialize};

use self::seal::Seal;
use crate::{Person, Verifier};

/// How long a started sign-in can be finished.
const PENDING_TTL: Duration = Duration::from_mins(10);

/// A session's absolute lifetime.
const SESSION_TTL: Duration = Duration::from_mins(15);

/// The most a token response may hold.
const BODY_LIMIT: u64 = 1 << 20;

/// Random bytes in a state and a CSRF token.
const RANDOM_BYTES: u32 = 32;

/// The cookie that carries a sign-in until the issuer sends the browser back.
pub(crate) const PENDING_COOKIE: &str = "moor_login";

/// What a cookie is sealed for, so one kind never opens as the other.
const PENDING_PURPOSE: &str = "pending";
/// See [`PENDING_PURPOSE`].
const SESSION_PURPOSE: &str = "session";

/// A sign-in waiting for the issuer to send the browser back, sealed in the browser's cookie.
#[derive(Serialize, Deserialize)]
struct Pending {
    /// The `state` the authorize URL carried; the callback must bring it back.
    state: String,
    /// The PKCE verifier.
    verifier: String,
    /// The same-origin path to land on.
    back: String,
    /// Where the code becomes tokens.
    token_endpoint: String,
}

/// A signed-in browser, sealed in its cookie.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    /// Who signed in.
    pub(crate) person: Person,
    /// The token forms on this session must post back.
    csrf: String,
}

/// Where the issuer signs people in and where codes become tokens.
#[derive(Clone)]
struct Endpoints {
    /// The authorize endpoint.
    authorize: String,
    /// The token endpoint.
    token: String,
}

/// An HTTP client that follows no redirect and hands back every status.
struct Http(Agent);

impl oauth2::SyncHttpClient for Http {
    type Error = ureq::Error;

    fn call(&self, request: oauth2::HttpRequest) -> Result<oauth2::HttpResponse, Self::Error> {
        let (parts, mut body) = self.0.run(request)?.into_parts();
        let body = body.with_config().limit(BODY_LIMIT).read_to_vec()?;
        Ok(ureq::http::Response::from_parts(parts, body))
    }
}

/// Browser sign-in for one public origin.
pub struct Login {
    /// The public URL, without a trailing slash.
    public: String,
    /// The machines pairing through this gate.
    pairings: pair::Pairings,
    /// `<public-url>/auth/callback`.
    redirect: String,
    /// Seals and opens the sign-in and session cookies.
    seal: Seal,
    /// The issuer's endpoints, once discovered.
    endpoints: Mutex<Option<Endpoints>>,
    /// Trades codes for tokens.
    http: Http,
}

impl Login {
    /// Sign-in for the gate served at `public_url`, such as `https://moor.example.com`, under
    /// a key of its own: cookies it seals open on this gate only.
    ///
    /// # Errors
    /// When `public_url` is not an `http` or `https` origin.
    pub fn new(public_url: &str) -> Result<Self, String> {
        Self::build(public_url, Seal::random())
    }

    /// Like [`Login::new`], with cookies sealed under `keys`, so every gate given the same keys
    /// serves each other's sign-ins. `keys` is one or more secrets of 32 or more characters
    /// separated by commas. The first seals and all open, which lets a key be replaced without
    /// ending sessions.
    ///
    /// # Errors
    /// When `public_url` is not an `http` or `https` origin, or a key is too short.
    pub fn with_keys(public_url: &str, keys: &str) -> Result<Self, String> {
        Self::build(public_url, Seal::from_keys(keys)?)
    }

    /// Sign-in for `public_url` sealing under `seal`.
    fn build(public_url: &str, seal: Seal) -> Result<Self, String> {
        let url = Url::parse(public_url).map_err(|e| format!("--public-url {public_url}: {e}"))?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            return Err(format!(
                "--public-url {public_url} is not an http(s) origin"
            ));
        }
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        let public = public_url.trim_end_matches('/').to_owned();
        Ok(Self {
            redirect: format!("{public}/auth/callback"),
            public,
            pairings: pair::Pairings::default(),
            seal,
            endpoints: Mutex::new(None),
            http: Http(Agent::new_with_config(config)),
        })
    }

    /// The issuer's endpoints, reading its discovery document the first time.
    fn endpoints(&self, verifier: &Verifier) -> Result<Endpoints, String> {
        let known = self
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(endpoints) = known {
            return Ok(endpoints);
        }
        let found = Endpoints {
            authorize: verifier.discover("authorization_endpoint")?,
            token: verifier.discover("token_endpoint")?,
        };
        *self
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(found.clone());
        Ok(found)
    }

    /// Starts a sign-in that lands on `back`: the issuer's authorize URL to send the browser
    /// to, and the sealed cookie value the callback needs.
    fn start(
        &self,
        verifier: &Verifier,
        back: &str,
        now: SystemTime,
    ) -> Result<(String, String), String> {
        let endpoints = self.endpoints(verifier)?;
        let (challenge, pkce) = PkceCodeChallenge::new_random_sha256();
        let state = CsrfToken::new_random_len(RANDOM_BYTES);
        let (url, _) = BasicClient::new(ClientId::new(verifier.client_id().to_owned()))
            .set_auth_uri(AuthUrl::new(endpoints.authorize).map_err(|e| e.to_string())?)
            .set_redirect_uri(RedirectUrl::new(self.redirect.clone()).map_err(|e| e.to_string())?)
            .authorize_url(|| state.clone())
            .add_scope(Scope::new("openid".to_owned()))
            .add_scope(Scope::new("email".to_owned()))
            .set_pkce_challenge(challenge)
            .url();
        let waiting = Pending {
            state: state.into_secret(),
            verifier: pkce.into_secret(),
            back: back.to_owned(),
            token_endpoint: endpoints.token,
        };
        let cookie = self
            .seal
            .seal(PENDING_PURPOSE, &waiting, now, PENDING_TTL)?;
        Ok((url.to_string(), cookie))
    }

    /// The sign-in the `Cookie` headers carry, when it is the one `state` names and has not
    /// expired.
    fn take<'a>(
        &self,
        cookies: impl IntoIterator<Item = &'a str>,
        state: &str,
        now: SystemTime,
    ) -> Option<Pending> {
        let sealed = cookie_value(cookies, PENDING_COOKIE)?;
        let pending: Pending = self.seal.open(PENDING_PURPOSE, sealed, now)?;
        same(&pending.state, state).then_some(pending)
    }

    /// Trades `code` for an access token, verifies it, and opens a session: its sealed cookie
    /// value and the path to land on.
    fn finish(
        &self,
        verifier: &Verifier,
        pending: Pending,
        code: &str,
        now: SystemTime,
    ) -> Result<(String, String), String> {
        let token = BasicClient::new(ClientId::new(verifier.client_id().to_owned()))
            .set_token_uri(TokenUrl::new(pending.token_endpoint).map_err(|e| e.to_string())?)
            .set_redirect_uri(RedirectUrl::new(self.redirect.clone()).map_err(|e| e.to_string())?)
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier))
            .request(&self.http)
            .map_err(|e| format!("the token request failed: {e}"))?;
        let person = verifier.verify(token.access_token().secret())?;
        Ok((self.open(person, now)?, pending.back))
    }

    /// The sealed cookie value of a new session for `person`.
    fn open(&self, person: Person, now: SystemTime) -> Result<String, String> {
        let session = Session {
            person,
            csrf: CsrfToken::new_random_len(RANDOM_BYTES).into_secret(),
        };
        self.seal.seal(SESSION_PURPOSE, &session, now, SESSION_TTL)
    }

    /// The live session the `Cookie` header values carry.
    pub(crate) fn session<'a>(
        &self,
        cookies: impl IntoIterator<Item = &'a str>,
        now: SystemTime,
    ) -> Option<Session> {
        let sealed = cookie_value(cookies, routes::COOKIE)?;
        self.seal.open(SESSION_PURPOSE, sealed, now)
    }
}

/// The value of cookie `name` among the `Cookie` header values.
fn cookie_value<'a>(cookies: impl IntoIterator<Item = &'a str>, name: &str) -> Option<&'a str> {
    cookies
        .into_iter()
        .flat_map(|header| header.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Whether two secrets are equal, compared in constant time.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token a form on `session` carries in its `csrf` field.
pub(crate) fn csrf_token(session: &Session) -> &str {
    &session.csrf
}

/// Whether a posted `csrf` field matches `session`'s token, compared in constant time.
pub(crate) fn check_csrf(session: &Session, value: &str) -> bool {
    same(&session.csrf, value)
}
