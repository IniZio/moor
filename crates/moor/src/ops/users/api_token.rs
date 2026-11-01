//! A user's API tokens: `api-token-create`, `api-token-list` and `api-token-revoke`, and the
//! `api-token-resolve` a front sends to learn whom a presented token speaks for.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use moor_core::request::{ApiTokenCmd, SecretValue};
use moor_core::user::{Caller, UserId};
use moor_core::view::user::{
    ApiTokenCreateValue, ApiTokenListValue, ApiTokenResolveValue, ApiTokenRevokeValue,
};
use moor_core::ErrorKind;
use sha2::{Digest as _, Sha256};

use super::to_value;
use crate::error::CommandError;
use crate::store::api_token::{NewApiToken, NotAdded};
use crate::store::Store;

/// What a token's text starts with.
pub(crate) const PREFIX: &str = "moor_";

/// The scopes a token may hold.
const SCOPES: [&str; 2] = ["moor:read", "moor:write"];

/// Most tokens one user may hold.
const MAX_TOKENS: usize = 20;

/// Longest token name, in characters.
const MAX_NAME: usize = 64;

/// The refusal for an id that is not the caller's, or does not exist.
const NO_SUCH: &str = "no such token";

/// The refusal for a token no one holds, or whose owner has no identity in the caller's
/// namespace; one message, so a caller cannot tell the two apart.
pub(crate) const UNKNOWN: &str = "unknown API token";

/// The refusal for any caller but a front.
pub(crate) const FRONT_ONLY: &str = "api-token-resolve serves only a front";

/// Run `cmd` for `user`.
pub(super) fn run(
    store: &Store,
    user: UserId,
    cmd: &ApiTokenCmd,
) -> Result<serde_json::Value, CommandError> {
    match cmd {
        ApiTokenCmd::Create { name, scopes } => create(store, user, name, scopes),
        ApiTokenCmd::List => to_value(&ApiTokenListValue {
            tokens: store.api_tokens(user)?,
        }),
        ApiTokenCmd::Revoke { id } => {
            if !store.delete_api_token(user, *id)? {
                return Err(CommandError::new(ErrorKind::Refused, NO_SUCH));
            }
            to_value(&ApiTokenRevokeValue { id: *id })
        }
    }
}

/// `api-token-create`: mint a token and store only its hash and last 4 characters.
fn create(
    store: &Store,
    user: UserId,
    name: &str,
    scopes: &[String],
) -> Result<serde_json::Value, CommandError> {
    let usage = |e: String| CommandError::new(ErrorKind::Usage, e);
    let length = name.chars().count();
    if length == 0 || length > MAX_NAME || name.chars().any(char::is_control) {
        return Err(usage(format!(
            "name: must be 1 to {MAX_NAME} printable characters"
        )));
    }
    let scopes = checked_scopes(scopes).map_err(usage)?;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("cannot mint a token: {e}")))?;
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hint = token.get(token.len() - 4..).unwrap_or_default();
    let new = NewApiToken {
        name,
        scopes: &scopes,
        hash: &hash(&token),
        hint,
    };
    let refused = |e: String| CommandError::new(ErrorKind::Refused, e);
    let id = match store.add_api_token(user, &new, MAX_TOKENS)? {
        Ok(id) => id,
        Err(NotAdded::Full) => {
            return Err(refused(format!("at most {MAX_TOKENS} tokens per user")))
        }
        Err(NotAdded::NameTaken) => return Err(refused(format!("a token named `{name}` exists"))),
    };
    to_value(&ApiTokenCreateValue {
        id,
        token: SecretValue::new(token.into_bytes()),
    })
}

/// `named` as a non-empty, deduplicated subset of [`SCOPES`], in their order.
fn checked_scopes(named: &[String]) -> Result<Vec<String>, String> {
    if let Some(bad) = named.iter().find(|s| !SCOPES.contains(&s.as_str())) {
        return Err(format!("scopes: `{bad}` is not one of {SCOPES:?}"));
    }
    let out: Vec<String> = SCOPES
        .iter()
        .filter(|s| named.iter().any(|n| n == *s))
        .map(|s| (*s).to_owned())
        .collect();
    if out.is_empty() {
        return Err(format!("scopes: name at least one of {SCOPES:?}"));
    }
    Ok(out)
}

/// The SHA-256 of `token`. A token holds 256 random bits, so a fast hash suffices: there is
/// nothing to guess, and a slow password hash would only slow every request.
fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// `api-token-resolve` for `caller`, which must be a front: the token owner's identity in that
/// front's namespace, their email, and the token's scopes.
pub(crate) fn resolve(
    store: &Store,
    caller: &Result<Caller, String>,
    token: &SecretValue,
) -> Result<serde_json::Value, CommandError> {
    let Ok(Caller::Front(ns)) = caller else {
        return Err(CommandError::new(ErrorKind::Refused, FRONT_ONLY));
    };
    let resolved = store
        .resolve_api_token(&Sha256::digest(token.secret().expose()), ns)?
        .ok_or_else(|| CommandError::new(ErrorKind::Refused, UNKNOWN))?;
    to_value(&ApiTokenResolveValue {
        identity: resolved.identity,
        email: resolved.email,
        scopes: resolved.scopes,
    })
}

#[cfg(test)]
mod tests;
