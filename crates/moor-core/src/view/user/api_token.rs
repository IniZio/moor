//! Values of the `api-token-*` requests.

use serde::{Deserialize, Serialize};

use crate::request::SecretValue;
use crate::user::{Email, Identity};

/// Value of `api-token-create`: the only reply that holds the token's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenCreateValue {
    /// The new token's id.
    pub id: i64,
    /// The token's text, `moor_` and 43 base64url characters.
    pub token: SecretValue,
}

/// One of a user's API tokens as `api-token-list` shows it; never its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenView {
    /// The token's id.
    pub id: i64,
    /// The name its person gave it.
    pub name: String,
    /// What it may do.
    pub scopes: Vec<String>,
    /// The token's last 4 characters, to tell tokens apart.
    pub hint: String,
    /// When it was created, in unix milliseconds.
    pub created_at: i64,
    /// When a front last resolved it, to the minute.
    pub last_used_at: Option<i64>,
}

/// Value of `api-token-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenListValue {
    /// The caller's tokens, oldest first.
    pub tokens: Vec<ApiTokenView>,
}

/// Value of `api-token-revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenRevokeValue {
    /// The id revoked.
    pub id: i64,
}

/// Value of `api-token-resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenResolveValue {
    /// The token owner's identity in the calling front's namespace.
    pub identity: Identity,
    /// The owner's verified email, when moor has one.
    pub email: Option<Email>,
    /// What the token may do.
    pub scopes: Vec<String>,
}
