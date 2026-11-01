//! Arguments of the `api-token-*` requests.

use serde::{Deserialize, Serialize};

use super::SecretValue;
use crate::user::Identity;

/// Arguments of `api-token-create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenCreateReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The name its person knows it by, unique among their tokens.
    pub name: String,
    /// What it may do: `moor:read`, `moor:write`, or both.
    pub scopes: Vec<String>,
}

/// Arguments of `api-token-revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenRevokeReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The token's id.
    pub id: i64,
}

/// Arguments of `api-token-resolve`, which a front sends for a token a client presented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenResolveReq {
    /// The token text the client presented.
    pub token: SecretValue,
}

/// A user's API-token command with its asserted identity taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiTokenCmd {
    /// Mint a token for the caller's user.
    Create {
        /// The token's name.
        name: String,
        /// The token's scopes.
        scopes: Vec<String>,
    },
    /// The caller's user's tokens, without their text.
    List,
    /// Revoke one of the caller's user's tokens.
    Revoke {
        /// The token's id.
        id: i64,
    },
}

impl ApiTokenCmd {
    /// The command name.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Create { .. } => "api-token-create",
            Self::List => "api-token-list",
            Self::Revoke { .. } => "api-token-revoke",
        }
    }
}
