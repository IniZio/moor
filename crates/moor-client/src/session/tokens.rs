//! Signing in and out, and the access token each dial presents.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use oauth2::basic::{BasicClient, BasicErrorResponseType};
use oauth2::url::form_urlencoded;
use oauth2::{ClientId, RefreshToken, RequestTokenError, SyncHttpClient, TokenResponse, TokenUrl};
use serde::Deserialize;

use super::{normalize, Credentials, Store};
use crate::oidc::{self, Http, Ui};
use crate::TokenSource;

/// Seconds before expiry from which an access token is refreshed.
const MARGIN: i64 = 300;

/// Lifetime assumed when the token endpoint names none.
const DEFAULT_LIFETIME: i64 = 3600;

/// Now, in unix seconds.
#[must_use]
#[expect(
    clippy::disallowed_methods,
    reason = "the one clock read of token expiry; tests pass their own"
)]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Seconds `expires_in` names, or the default.
fn lifetime(expires_in: Option<std::time::Duration>) -> i64 {
    expires_in.map_or(DEFAULT_LIFETIME, |d| {
        i64::try_from(d.as_secs()).unwrap_or(DEFAULT_LIFETIME)
    })
}

/// The `email` claim of an id token. Only read for display, so the signature is not checked.
fn email_in(id_token: &str) -> Option<String> {
    /// The one claim read.
    #[derive(Deserialize)]
    struct Claims {
        /// Who signed in.
        email: Option<String>,
    }
    let payload = URL_SAFE_NO_PAD.decode(id_token.split('.').nth(1)?).ok()?;
    serde_json::from_slice::<Claims>(&payload).ok()?.email
}

/// Signs in to `server` and saves the sign-in in `store`. `port` is the loopback port for the
/// browser's return; 0 takes any free one. `now` is unix seconds.
///
/// # Errors
/// The sign-in fails, grants no refresh token, or cannot be saved.
pub fn login(
    server: &str,
    port: u16,
    store: &Store,
    ui: &mut dyn Ui,
    now: i64,
) -> Result<Credentials, String> {
    let signed = oidc::sign_in(&normalize(server), port, ui)?;
    let refresh_token = signed.refresh_token.ok_or_else(|| {
        "the sign-in returned no refresh token; offline access is needed".to_owned()
    })?;
    let creds = Credentials {
        server: normalize(&signed.server),
        issuer: signed.issuer,
        client_id: signed.client_id,
        token_endpoint: signed.token_endpoint,
        refresh_token,
        access_token: signed.access_token,
        expires_at: now + lifetime(signed.expires_in),
        email: signed.id_token.as_deref().and_then(email_in),
    };
    store
        .write(&creds)
        .map_err(|e| format!("cannot save the credentials: {e}"))?;
    Ok(creds)
}

/// The issuer's revocation endpoint, from its discovery document.
#[derive(Deserialize)]
struct Revocation {
    /// Where tokens are revoked; absent when the issuer offers none.
    revocation_endpoint: Option<String>,
}

/// Forgets the sign-in to `server` and revokes its refresh token at the issuer, best effort.
/// True when there was a sign-in.
///
/// # Errors
/// The credentials cannot be read or written.
pub fn logout(store: &Store, server: &str) -> Result<bool, String> {
    let Some(creds) = store.read(server)? else {
        return Ok(false);
    };
    revoke(&creds);
    store.delete(server).map_err(|e| e.to_string())
}

/// Revokes `creds`' refresh token, ignoring any failure: the sign-in is forgotten either way.
/// Posted by hand, since the oauth2 crate refuses a revocation URL that is not https.
fn revoke(creds: &Credentials) {
    let http = Http::new();
    let at = format!(
        "{}/.well-known/openid-configuration",
        creds.issuer.trim_end_matches('/')
    );
    let Ok(Revocation {
        revocation_endpoint: Some(endpoint),
    }) = http.get_json(&at)
    else {
        return;
    };
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("token", &creds.refresh_token)
        .append_pair("token_type_hint", "refresh_token")
        .append_pair("client_id", &creds.client_id)
        .finish();
    if let Ok(request) = oauth2::http::Request::post(endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.into_bytes())
    {
        let _ = http.call(request);
    }
}

/// The access token for one server, from the saved sign-in.
#[derive(Debug)]
pub struct Tokens {
    /// Where the sign-in is saved.
    store: Store,
    /// The server, normalized.
    server: String,
    /// Unix seconds now.
    clock: fn() -> i64,
    /// The command that signs in, named when there is no sign-in.
    login: String,
}

impl Tokens {
    /// Tokens for `server` from `store`.
    #[must_use]
    pub fn new(store: Store, server: &str) -> Self {
        Self::with_clock(store, server, unix_now)
    }

    /// Tokens that read the time from `clock`.
    #[must_use]
    pub fn with_clock(store: Store, server: &str, clock: fn() -> i64) -> Self {
        Self {
            store,
            server: normalize(server),
            clock,
            login: "moor login".to_owned(),
        }
    }

    /// Names `command` instead of `moor login` when the person must sign in.
    #[must_use]
    pub fn login_with(self, command: &str) -> Self {
        Self {
            login: command.to_owned(),
            ..self
        }
    }

    /// What to run to sign in.
    fn hint(&self) -> String {
        format!("run `{} --server {}`", self.login, self.server)
    }

    /// The saved sign-in for this server.
    fn saved(&self) -> Result<Credentials, String> {
        self.store
            .read(&self.server)?
            .ok_or_else(|| format!("not signed in to {}; {}", self.server, self.hint()))
    }

    /// Whether `creds` has more than the refresh margin left.
    fn fresh(&self, creds: &Credentials) -> bool {
        creds.expires_at - (self.clock)() > MARGIN
    }

    /// An access token other than `rejected`, which the server refused: the saved one when
    /// another process already replaced it, else one refreshed now.
    ///
    /// # Errors
    /// Says why there is none, such as that the sign-in ended.
    pub fn renewed(&self, rejected: &str) -> Result<String, String> {
        self.refreshed(|c| c.access_token != rejected)
            .map(|c| c.access_token)
    }

    /// The saved sign-in, refreshed under the lock unless `keep` accepts it as saved.
    fn refreshed(&self, keep: impl Fn(&Credentials) -> bool) -> Result<Credentials, String> {
        let _lock = self
            .store
            .lock()
            .map_err(|e| format!("cannot lock the credentials: {e}"))?;
        let creds = self.saved()?;
        if keep(&creds) {
            return Ok(creds);
        }
        let next = self.refresh(&creds)?;
        self.store
            .put(&next)
            .map_err(|e| format!("cannot save the credentials: {e}"))?;
        Ok(next)
    }

    /// `creds` with the tokens the token endpoint returns for its refresh token.
    fn refresh(&self, creds: &Credentials) -> Result<Credentials, String> {
        let token_url = TokenUrl::new(creds.token_endpoint.clone()).map_err(|e| e.to_string())?;
        let answer = BasicClient::new(ClientId::new(creds.client_id.clone()))
            .set_token_uri(token_url)
            .exchange_refresh_token(&RefreshToken::new(creds.refresh_token.clone()))
            .request(&Http::new());
        let token = match answer {
            Ok(token) => token,
            Err(RequestTokenError::ServerResponse(r))
                if *r.error() == BasicErrorResponseType::InvalidGrant =>
            {
                return Err(format!(
                    "the sign-in to {} ended; {}",
                    self.server,
                    self.hint()
                ));
            }
            Err(e) => {
                return Err(format!(
                    "refreshing the sign-in at {} failed: {e}",
                    creds.token_endpoint
                ))
            }
        };
        Ok(Credentials {
            access_token: token.access_token().secret().clone(),
            refresh_token: token
                .refresh_token()
                .map_or_else(|| creds.refresh_token.clone(), |t| t.secret().clone()),
            expires_at: (self.clock)() + lifetime(token.expires_in()),
            ..creds.clone()
        })
    }
}

impl TokenSource for Tokens {
    fn bearer(&self) -> Result<String, String> {
        let creds = self.saved()?;
        if self.fresh(&creds) {
            return Ok(creds.access_token);
        }
        self.refreshed(|c| self.fresh(c)).map(|c| c.access_token)
    }
}
