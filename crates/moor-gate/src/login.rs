//! Browser sign-in: the authorization-code flow with PKCE against the issuer, and sessions
//! that last while the person is active, for pages that must only reach a signed-in person.
//! A started sign-in and a session both live in sealed cookies, so any gate replica sharing
//! the key can serve them, and a restart that keeps the key ends no session.

pub(crate) mod account;
mod cookie;
mod kept;
mod nodes;
pub(crate) mod pair;
mod portal;
mod recheck;
mod repos;
pub(crate) mod routes;
mod seal;
pub(crate) mod secrets;
mod settings;
pub(crate) mod setup;
pub(crate) mod terminal;
#[cfg(test)]
mod tests;
mod workflows;

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

use self::cookie::Issued;
use self::seal::Seal;
use crate::{Person, Verifier};

/// How long a started sign-in can be finished.
const PENDING_TTL: Duration = Duration::from_mins(10);

/// How long a session lasts after its last renewal: the page renews it while the person is active.
const SESSION_IDLE: Duration = Duration::from_hours(12);

/// The most a session lasts from sign-in, however often it is renewed.
const SESSION_MAX: Duration = Duration::from_hours(24);

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
    /// Unix seconds of the sign-in, which bounds every renewal by [`SESSION_MAX`].
    since: u64,
    /// The issuer's refresh token, traded at each check to ask whether the person may still
    /// sign in. None when the issuer gave none; such a session is never checked.
    #[serde(default)]
    refresh: Option<String>,
    /// Unix seconds from which the session opens only for a check, see [`Login::recheck`].
    #[serde(default)]
    due: u64,
    /// Unix seconds of the issuer's last yes.
    #[serde(default)]
    vouched: u64,
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
    /// `<public-url>/auth/callback`.
    redirect: String,
    /// Seals and opens the sign-in and session cookies.
    seal: Seal,
    /// The issuer's endpoints, once discovered.
    endpoints: Mutex<Option<Endpoints>>,
    /// Trades codes for tokens.
    http: Http,
    /// Seconds a pairing agent is told to wait between polls.
    pair_interval: u64,
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

    /// Like [`Login::new`], under a key kept in the file at `path`, made there when missing, so
    /// sessions outlive a restart of a gate that runs alone.
    ///
    /// # Errors
    /// When `public_url` is not an `http` or `https` origin, or the file cannot be read or made.
    pub fn with_kept_key(public_url: &str, path: &std::path::Path) -> Result<Self, String> {
        Self::build(public_url, Seal::from_keys(&kept::read_or_make(path)?)?)
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
            seal,
            endpoints: Mutex::new(None),
            http: Http(Agent::new_with_config(config)),
            pair_interval: pair::INTERVAL_SECS,
        })
    }

    /// Tells pairing agents to poll every `secs` seconds instead of the default, so a test
    /// that pairs does not wait out the default.
    #[must_use]
    pub const fn with_pair_interval(mut self, secs: u64) -> Self {
        self.pair_interval = secs;
        self
    }

    /// The public origin, without a trailing slash.
    pub(crate) fn public(&self) -> &str {
        &self.public
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
            .add_scope(Scope::new("offline_access".to_owned()))
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
    ) -> Result<(Issued, String), String> {
        let token = BasicClient::new(ClientId::new(verifier.client_id().to_owned()))
            .set_token_uri(TokenUrl::new(pending.token_endpoint).map_err(|e| e.to_string())?)
            .set_redirect_uri(RedirectUrl::new(self.redirect.clone()).map_err(|e| e.to_string())?)
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier))
            .request(&self.http)
            .map_err(|e| format!("the token request failed: {e}"))?;
        let person = verifier.verify(token.access_token().secret())?;
        let refresh = token.refresh_token().map(|r| r.secret().clone());
        if refresh.is_none() {
            crate::log("the issuer gave no refresh token, so this session is never checked again");
        }
        Ok((self.open(person, refresh, now)?, pending.back))
    }

    /// The cookie of a new session for `person`, checked with `refresh` from now on.
    fn open(
        &self,
        person: Person,
        refresh: Option<String>,
        now: SystemTime,
    ) -> Result<Issued, String> {
        let at = seal::unix(now);
        let session = Session {
            person,
            csrf: CsrfToken::new_random_len(RANDOM_BYTES).into_secret(),
            since: at,
            refresh,
            due: at.saturating_add(recheck::EVERY.as_secs()),
            vouched: at,
        };
        self.renew(&session, now)
    }

    /// `session` sealed again to last [`SESSION_IDLE`] from `now`, never past [`SESSION_MAX`]
    /// from its sign-in. Its person and CSRF token stay, so open pages keep working.
    pub(crate) fn renew(&self, session: &Session, now: SystemTime) -> Result<Issued, String> {
        let end = session.since.saturating_add(SESSION_MAX.as_secs());
        let left = Duration::from_secs(end.saturating_sub(seal::unix(now)));
        let ttl = SESSION_IDLE.min(left);
        self.issue(session, now, ttl)
    }

    /// `session` sealed again to end when it would have, at Unix second `until`.
    pub(crate) fn keep(
        &self,
        session: &Session,
        until: u64,
        now: SystemTime,
    ) -> Result<Issued, String> {
        let ttl = Duration::from_secs(until.saturating_sub(seal::unix(now)));
        self.issue(session, now, ttl)
    }

    /// `session` sealed to last `ttl` from `now`.
    fn issue(&self, session: &Session, now: SystemTime, ttl: Duration) -> Result<Issued, String> {
        if ttl.is_zero() {
            return Err("the session reached its longest life".to_owned());
        }
        let sealed = self.seal.seal(SESSION_PURPOSE, session, now, ttl)?;
        Ok(Issued { sealed, ttl })
    }

    /// The live session the `Cookie` header values carry, unless its check is due: then only
    /// `/auth/check` and `/auth/renew` open it, through [`Login::opened`].
    pub(crate) fn session<'a>(
        &self,
        cookies: impl IntoIterator<Item = &'a str>,
        now: SystemTime,
    ) -> Option<Session> {
        let (session, _) = self.opened(cookies, now)?;
        (session.refresh.is_none() || seal::unix(now) < session.due).then_some(session)
    }

    /// The unexpired session the `Cookie` header values carry, checked or not, and the Unix
    /// second it ends.
    pub(crate) fn opened<'a>(
        &self,
        cookies: impl IntoIterator<Item = &'a str>,
        now: SystemTime,
    ) -> Option<(Session, u64)> {
        let sealed = cookie_value(cookies, routes::COOKIE)?;
        self.seal.open_until(SESSION_PURPOSE, sealed, now)
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
