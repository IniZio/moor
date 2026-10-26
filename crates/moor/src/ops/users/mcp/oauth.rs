//! Signing a user in to one of their own MCP servers over OAuth. The controller is the client:
//! it discovers the server's authorization server, registers itself unless the operator named
//! a client, and keeps the access token as the server's vault secret so the Proxy injects it.

mod http;
pub(crate) mod refresh;

use std::time::Duration;

use moor_core::config::{OAuthConfig, UserMcp};
use moor_core::request::UserCmd;
use moor_core::user::{SecretName, UserId};
use moor_core::view::user::McpChangeValue;
use moor_core::{ErrorKind, Secret};
use oauth2::basic::BasicClient;
use oauth2::url::Url;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use self::http::Http;
use super::super::{set_secret, to_value};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::vault::Vault;

/// How long a started sign-in can be finished.
const PENDING_TTL: Duration = Duration::from_secs(600);

/// The redirect URI when the operator set no `oauth.callback_url`. The browser may fail to
/// load it; the user pastes the address it ended on.
const LOOPBACK_REDIRECT: &str = "http://127.0.0.1:33418/oauth/callback";

/// The client the controller signs in as.
#[derive(Serialize, Deserialize)]
struct Client {
    /// The client id.
    id: String,
    /// The secret a registration issued; an operator's client keeps its secret in config.
    secret: Option<String>,
}

/// A sign-in waiting for its code, sealed in the store.
#[derive(Serialize, Deserialize)]
struct Pending {
    /// The PKCE verifier.
    verifier: String,
    /// The authorization server the sign-in went to.
    issuer: String,
    /// Whether that server names itself in `iss` on the redirect.
    iss_required: bool,
    /// Where the code becomes tokens.
    token_endpoint: String,
    /// The redirect URI the authorize request named.
    redirect: String,
    /// The MCP server's URL, sent as `resource`.
    resource: String,
    /// The client signed in as.
    client: Client,
}

/// What a signed-in server keeps beside its access token. The Proxy never reads it.
#[derive(Serialize, Deserialize)]
struct Grant {
    /// The refresh token, when the server issued one.
    refresh_token: Option<String>,
    /// The authorization server.
    issuer: String,
    /// Where tokens are refreshed.
    token_endpoint: String,
    /// The MCP server's URL, sent as `resource`.
    resource: String,
    /// The client signed in as.
    client: Client,
}

/// Starts signing `user` in to `server` when it answers 401 naming resource metadata: the
/// address the user opens, or `None` when the server does not ask for OAuth.
///
/// # Errors
/// Discovery or registration fails, or the store cannot be written.
pub(super) fn start(
    cx: &Cx,
    vault: &Vault,
    user: UserId,
    server: &UserMcp,
) -> Result<Option<String>, CommandError> {
    let http = Http::new();
    let Some(at) = http.probe(&server.url) else {
        return Ok(None);
    };
    let resource = http.resource(&at, &server.url).map_err(failed)?;
    let issuer = resource
        .authorization_servers
        .first()
        .ok_or_else(|| failed(format!("{at} names no authorization server")))?;
    let meta = http.server(issuer).map_err(failed)?;
    if !meta
        .code_challenge_methods_supported
        .iter()
        .any(|m| m == "S256")
    {
        return Err(failed(format!("{issuer} does not offer PKCE S256")));
    }
    let oauth = &cx.config().oauth;
    let redirect = oauth
        .callback_url
        .clone()
        .unwrap_or_else(|| LOOPBACK_REDIRECT.to_owned());
    let client = client_for(&http, oauth, &meta, &redirect)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = BasicClient::new(ClientId::new(client.id.clone()))
        .set_auth_uri(AuthUrl::new(meta.authorization_endpoint.clone()).map_err(failed)?)
        .set_redirect_uri(RedirectUrl::new(redirect.clone()).map_err(failed)?)
        .authorize_url(CsrfToken::new_random)
        .add_scopes(resource.scopes_supported.iter().cloned().map(Scope::new))
        .add_extra_param("resource", server.url.as_str())
        .set_pkce_challenge(challenge)
        .url();
    let pending = Pending {
        verifier: verifier.into_secret(),
        issuer: meta.issuer,
        iss_required: meta.authorization_response_iss_parameter_supported,
        token_endpoint: meta.token_endpoint,
        redirect,
        resource: server.url.clone(),
        client,
    };
    let sealed = seal(vault, user, &server.secret(), &pending)?;
    let expires = cx.clock().now().saturating_add(PENDING_TTL);
    cx.store()?
        .add_mcp_oauth_pending(state.secret(), user, (&server.name, &sealed), expires)?;
    Ok(Some(url.to_string()))
}

/// The operator's client for `meta`'s issuer, or one registered there now.
fn client_for(
    http: &Http,
    oauth: &OAuthConfig,
    meta: &http::Server,
    redirect: &str,
) -> Result<Client, CommandError> {
    if let Some(c) = operator_client(oauth, &meta.issuer) {
        return Ok(Client {
            id: c.client_id.clone(),
            secret: None,
        });
    }
    let endpoint = meta.registration_endpoint.as_deref().ok_or_else(|| {
        failed(format!(
            "{} offers no registration and the operator named no client for it",
            meta.issuer
        ))
    })?;
    let r = http.register(endpoint, redirect).map_err(failed)?;
    Ok(Client {
        id: r.client_id,
        secret: r.client_secret,
    })
}

/// `mcp-oauth-finish`: trades the code of `user`'s sign-in under `state` for tokens. The
/// access token becomes the server's secret; the rest is sealed apart from it.
///
/// # Errors
/// No unexpired sign-in of `user`'s waits under `state`, `iss` is not its issuer, or the
/// token request fails.
pub(crate) fn finish(
    cx: &Cx,
    vault: &Vault,
    user: UserId,
    cmd: &UserCmd,
) -> Result<serde_json::Value, CommandError> {
    let UserCmd::McpOAuthFinish { state, code, iss } = cmd else {
        return Err(usage("not a sign-in to finish"));
    };
    let store = cx.store()?;
    let row = store
        .take_mcp_oauth_pending(state, user)?
        .ok_or_else(|| usage("no sign-in of yours waits under that state; add the server again"))?;
    if row.expires <= cx.clock().now() {
        return Err(usage("that sign-in expired; add the server again"));
    }
    let secret = UserMcp::parse(&row.name, "https://placeholder.invalid", None)
        .map_err(failed)?
        .secret();
    let p: Pending = open(vault, user, &secret, &row.sealed)?;
    check_iss(&p.issuer, p.iss_required, iss.as_deref())?;
    let token = client(cx.config(), &p.client, &p.issuer)
        .set_token_uri(TokenUrl::new(p.token_endpoint.clone()).map_err(failed)?)
        .set_redirect_uri(RedirectUrl::new(p.redirect).map_err(failed)?)
        .exchange_code(AuthorizationCode::new(code.clone()))
        .set_pkce_verifier(PkceCodeVerifier::new(p.verifier))
        .add_extra_param("resource", p.resource.clone())
        .request(&Http::new())
        .map_err(|e| {
            failed(format!(
                "the token request to {} failed: {e}",
                p.token_endpoint
            ))
        })?;
    let access = token.access_token().secret().as_bytes().to_vec();
    set_secret(store, vault, user, &secret, &Secret::new(access))?;
    let grant = Grant {
        refresh_token: token.refresh_token().map(|t| t.secret().clone()),
        issuer: p.issuer,
        token_endpoint: p.token_endpoint,
        resource: p.resource,
        client: p.client,
    };
    let now = cx.clock().now();
    let expires = token.expires_in().map(|d| now.saturating_add(d));
    let sealed = seal(vault, user, &secret, &grant)?;
    store.set_user_mcp_oauth(user, (&row.name, &sealed), (now, expires))?;
    to_value(&McpChangeValue {
        name: row.name,
        changed: true,
        secret,
        sign_in: None,
    })
}

/// RFC 9207: `iss` must name the issuer the sign-in went to, and must be there when that
/// issuer said it sends one.
fn check_iss(issuer: &str, required: bool, iss: Option<&str>) -> Result<(), CommandError> {
    match iss {
        Some(got) if got == issuer => Ok(()),
        Some(got) => Err(usage(format!(
            "the sign-in came back from {got}, not {issuer}; add the server again"
        ))),
        None if required => Err(usage(format!(
            "the sign-in came back without naming its issuer {issuer}; add the server again"
        ))),
        None => Ok(()),
    }
}

/// The OAuth client for `c` at `issuer`, with the operator's secret when the operator
/// registered it.
fn client(config: &moor_core::config::UserConfig, c: &Client, issuer: &str) -> BasicClient {
    let secret = c.secret.clone().or_else(|| {
        operator_client(&config.oauth, issuer)?
            .client_secret
            .clone()
    });
    let client = BasicClient::new(ClientId::new(c.id.clone()));
    match secret {
        Some(s) => client.set_client_secret(ClientSecret::new(s)),
        None => client,
    }
}

/// The client the operator registered with `issuer`'s host, if any.
fn operator_client<'a>(
    oauth: &'a OAuthConfig,
    issuer: &str,
) -> Option<&'a moor_core::config::OAuthClient> {
    let url = Url::parse(issuer).ok()?;
    oauth.clients.get(url.host_str()?)
}

/// `value` as JSON sealed to `user`'s server secret `name`.
fn seal<T: Serialize>(
    vault: &Vault,
    user: UserId,
    name: &SecretName,
    value: &T,
) -> Result<Vec<u8>, CommandError> {
    let json = serde_json::to_vec(value).map_err(failed)?;
    vault.seal(user, name, &Secret::new(json)).map_err(failed)
}

/// The record [`seal`] sealed.
fn open<T: DeserializeOwned>(
    vault: &Vault,
    user: UserId,
    name: &SecretName,
    sealed: &[u8],
) -> Result<T, CommandError> {
    let plain = vault.open(user, name, sealed).map_err(failed)?;
    serde_json::from_slice(plain.expose()).map_err(failed)
}

/// A refusal the caller can act on.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// A failure on the far side or in the controller.
fn failed(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Failed, why.to_string())
}

#[cfg(test)]
mod tests;
