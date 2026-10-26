//! Keeping signed-in servers' access tokens fresh. The controller refreshes each one ahead of
//! its expiry; a grant the authorization server no longer honours ends the sign-in.

use std::collections::HashMap;
use std::time::Duration;

use moor_core::config::UserMcp;
use moor_core::seam::clock::Ts;
use moor_core::Secret;
use oauth2::basic::BasicErrorResponseType;
use oauth2::{RefreshToken, RequestTokenError, TokenResponse, TokenUrl};

use super::http::Http;
use super::{client, failed, open, seal, Grant};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::users::set_secret;
use crate::store::GrantRow;
use crate::vault::Vault;

/// The least time before expiry a refresh starts; a long-lived token starts at a tenth of its
/// lifetime instead.
const MARGIN: Duration = Duration::from_mins(5);

/// The wait after a first failed refresh; it doubles with each failure up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(30);

/// The longest wait between failed refreshes.
const MAX_RETRY: Duration = Duration::from_mins(15);

/// Refreshes that failed for now, by user and server: failures so far and the next try.
#[derive(Debug, Default)]
pub(crate) struct Retries(HashMap<(i64, String), (u32, Ts)>);

/// What one refresh did.
enum Outcome {
    /// A new access token is the server's secret.
    Refreshed,
    /// The server gave no refresh token and the access token still works.
    LeftAlone,
    /// The sign-in ended; the user must sign in again.
    Ended,
}

/// Refreshes every access token near its expiry, one at a time; returns lines to log. A
/// failure the server may recover from waits out a backoff in `retries` and keeps the token.
pub(crate) fn refresh_due(cx: &Cx, vault: &Vault, retries: &mut Retries) -> Vec<String> {
    let rows = match cx.store().and_then(|s| Ok(s.expiring_mcp_oauth()?)) {
        Ok(rows) => rows,
        Err(e) => return vec![format!("oauth: {}", e.error)],
    };
    let now = cx.clock().now();
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| due(now, r)) {
        let key = (row.user.get(), row.name.clone());
        if retries.0.get(&key).is_some_and(|(_, at)| *at > now) {
            continue;
        }
        let who = format!("user {} MCP server {}", row.user.get(), row.name);
        match refresh(cx, vault, row, now) {
            Ok(Outcome::Refreshed | Outcome::LeftAlone) => {
                retries.0.remove(&key);
            }
            Ok(Outcome::Ended) => {
                retries.0.remove(&key);
                out.push(format!("oauth: {who} needs sign-in"));
            }
            Err(e) => {
                let failures = retries.0.get(&key).map_or(0, |(n, _)| *n);
                let wait = FIRST_RETRY
                    .saturating_mul(1 << failures.min(5))
                    .min(MAX_RETRY);
                retries
                    .0
                    .insert(key, (failures + 1, now.saturating_add(wait)));
                out.push(format!(
                    "oauth: refreshing {who} failed, retrying in {}s: {}",
                    wait.as_secs(),
                    e.error
                ));
            }
        }
    }
    out
}

/// Whether `row`'s token is within the larger of [`MARGIN`] and a tenth of its lifetime of
/// expiring.
fn due(now: Ts, row: &GrantRow) -> bool {
    let lifetime = row
        .expires
        .as_millis()
        .saturating_sub(row.issued.as_millis());
    let margin = (lifetime / 10).max(i64::try_from(MARGIN.as_millis()).unwrap_or(i64::MAX));
    now.as_millis() >= row.expires.as_millis().saturating_sub(margin)
}

/// Trades `row`'s refresh token for a new access token, or ends the sign-in when the grant is
/// refused or the token expired with nothing to refresh it.
///
/// # Errors
/// A failure that may pass: the network, the server, or the store.
fn refresh(cx: &Cx, vault: &Vault, row: &GrantRow, now: Ts) -> Result<Outcome, CommandError> {
    let store = cx.store()?;
    let secret = UserMcp::parse(&row.name, "https://placeholder.invalid", None)
        .map_err(failed)?
        .secret();
    let mut grant: Grant = open(vault, row.user, &secret, &row.sealed)?;
    let end = || -> Result<Outcome, CommandError> {
        store.end_user_mcp_oauth(row.user, &row.name)?;
        store.delete_user_secret(row.user, &secret)?;
        Ok(Outcome::Ended)
    };
    let Some(refresh) = grant.refresh_token.clone() else {
        return if row.expires <= now {
            end()
        } else {
            Ok(Outcome::LeftAlone)
        };
    };
    let answer = client(cx.config(), &grant.client, &grant.issuer)
        .set_token_uri(TokenUrl::new(grant.token_endpoint.clone()).map_err(failed)?)
        .exchange_refresh_token(&RefreshToken::new(refresh))
        .add_extra_param("resource", grant.resource.clone())
        .request(&Http::new());
    let token = match answer {
        Ok(token) => token,
        Err(RequestTokenError::ServerResponse(r))
            if *r.error() == BasicErrorResponseType::InvalidGrant =>
        {
            return end();
        }
        Err(e) => {
            return Err(failed(format!(
                "the token request to {} failed: {e}",
                grant.token_endpoint
            )))
        }
    };
    if let Some(rotated) = token.refresh_token() {
        grant.refresh_token = Some(rotated.secret().clone());
    }
    let expires = token.expires_in().map(|d| now.saturating_add(d));
    let sealed = seal(vault, row.user, &secret, &grant)?;
    if store.refresh_user_mcp_oauth(row, &sealed, (now, expires))? {
        let access = token.access_token().secret().as_bytes().to_vec();
        set_secret(store, vault, row.user, &secret, &Secret::new(access))?;
    }
    Ok(Outcome::Refreshed)
}
