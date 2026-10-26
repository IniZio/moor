//! Arguments of the `mcp-*` requests.

use serde::{Deserialize, Serialize};

use super::SecretValue;
use crate::user::Identity;

/// Arguments of `mcp-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The server's name.
    pub name: String,
    /// The server's `https://` URL.
    pub url: String,
    /// The header the key goes in; `Authorization` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// The key, stored in the vault; the stored one is kept when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<SecretValue>,
    /// Whether only a Box whose repo asks for a machine gets the server.
    #[serde(default)]
    pub machine: bool,
}

/// Arguments of `mcp-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpRemoveReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The server's name.
    pub name: String,
}

/// Arguments of `mcp-oauth-finish`: what the authorization server sent to the redirect URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthFinishReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The `state` the sign-in started with.
    pub state: String,
    /// The authorization code.
    pub code: String,
    /// The issuer the authorization server named, when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
}
