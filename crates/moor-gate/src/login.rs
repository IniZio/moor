//! Browser sign-in: the authorization-code flow with PKCE against the issuer, and short
//! in-memory sessions keyed by a cookie, for pages that must only reach a signed-in person.

pub(crate) mod routes;
pub(crate) mod secrets;
pub(crate) mod setup;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oauth2::basic::BasicClient;
use oauth2::url::Url;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use crate::{Person, Verifier};

/// How long a started sign-in can be finished.
const PENDING_TTL: Duration = Duration::from_mins(10);

/// The most sign-ins waiting at once; the oldest goes first.
const PENDING_MAX: usize = 1000;

/// A session's absolute lifetime.
const SESSION_TTL: Duration = Duration::from_mins(15);

/// The most a token response may hold.
const BODY_LIMIT: u64 = 1 << 20;

/// Random bytes in a state, a session id and a CSRF token.
const RANDOM_BYTES: u32 = 32;

/// A sign-in waiting for the issuer to send the browser back.
struct Pending {
    /// The PKCE verifier.
    verifier: String,
    /// The same-origin path to land on.
    back: String,
    /// Where the code becomes tokens.
    token_endpoint: String,
    /// When it started.
    created: Instant,
}

/// A signed-in browser.
#[derive(Clone)]
pub(crate) struct Session {
    /// The cookie value.
    pub(crate) id: String,
    /// Who signed in.
    pub(crate) person: Person,
    /// The token forms on this session must post back.
    csrf: String,
    /// When it ends, however busy it is.
    expires: Instant,
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
    /// `<public-url>/auth/callback`.
    redirect: String,
    /// Sign-ins waiting, by state.
    pending: Mutex<HashMap<String, Pending>>,
    /// Sessions, by id.
    sessions: Mutex<HashMap<String, Session>>,
    /// The issuer's endpoints, once discovered.
    endpoints: Mutex<Option<Endpoints>>,
    /// Trades codes for tokens.
    http: Http,
}

impl Login {
    /// Sign-in for the gate served at `public_url`, such as `https://moor.example.com`.
    ///
    /// # Errors
    /// When `public_url` is not an `http` or `https` origin.
    pub fn new(public_url: &str) -> Result<Self, String> {
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
        Ok(Self {
            redirect: format!("{}/auth/callback", public_url.trim_end_matches('/')),
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
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

    /// Starts a sign-in that lands on `back`: the issuer's authorize URL to send the browser to.
    fn start(&self, verifier: &Verifier, back: &str, now: Instant) -> Result<String, String> {
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
            verifier: pkce.into_secret(),
            back: back.to_owned(),
            token_endpoint: endpoints.token,
            created: now,
        };
        self.remember(state.into_secret(), waiting, now);
        Ok(url.to_string())
    }

    /// Keeps `pending` under `state`, dropping expired sign-ins and then the oldest past the cap.
    fn remember(&self, state: String, pending: Pending, now: Instant) {
        let mut all = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|_, p| now.saturating_duration_since(p.created) < PENDING_TTL);
        while all.len() >= PENDING_MAX {
            let oldest = all
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(key) => all.remove(&key),
                None => break,
            };
        }
        all.insert(state, pending);
    }

    /// Takes the sign-in `state` names, once, when it has not expired.
    fn take(&self, state: &str, now: Instant) -> Option<Pending> {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(state)?;
        (now.saturating_duration_since(pending.created) < PENDING_TTL).then_some(pending)
    }

    /// Trades `code` for an access token, verifies it, and opens a session: it and the path
    /// to land on.
    fn finish(
        &self,
        verifier: &Verifier,
        pending: Pending,
        code: &str,
        now: Instant,
    ) -> Result<(Session, String), String> {
        let token = BasicClient::new(ClientId::new(verifier.client_id().to_owned()))
            .set_token_uri(TokenUrl::new(pending.token_endpoint).map_err(|e| e.to_string())?)
            .set_redirect_uri(RedirectUrl::new(self.redirect.clone()).map_err(|e| e.to_string())?)
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier))
            .request(&self.http)
            .map_err(|e| format!("the token request failed: {e}"))?;
        let person = verifier.verify(token.access_token().secret())?;
        Ok((self.open(person, now), pending.back))
    }

    /// Opens a session for `person`.
    fn open(&self, person: Person, now: Instant) -> Session {
        let session = Session {
            id: CsrfToken::new_random_len(RANDOM_BYTES).into_secret(),
            person,
            csrf: CsrfToken::new_random_len(RANDOM_BYTES).into_secret(),
            expires: now + SESSION_TTL,
        };
        let mut all = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|_, s| s.expires > now);
        all.insert(session.id.clone(), session.clone());
        session
    }

    /// The live session the `Cookie` header values name.
    pub(crate) fn session<'a>(
        &self,
        cookies: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> Option<Session> {
        let id = cookies
            .into_iter()
            .flat_map(|header| header.split(';'))
            .find_map(|pair| pair.trim().strip_prefix(routes::COOKIE)?.strip_prefix('='))?;
        let all = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        all.get(id).filter(|s| s.expires > now).cloned()
    }

    /// Ends `session`.
    fn close(&self, session: &Session) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session.id);
    }
}

/// The token a form on `session` carries in its `csrf` field.
pub(crate) fn csrf_token(session: &Session) -> &str {
    &session.csrf
}

/// Whether a posted `csrf` field matches `session`'s token, compared in constant time.
pub(crate) fn check_csrf(session: &Session, value: &str) -> bool {
    let (want, got) = (session.csrf.as_bytes(), value.as_bytes());
    want.len() == got.len() && want.iter().zip(got).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}
