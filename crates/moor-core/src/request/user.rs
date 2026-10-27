//! Requests about moor users. Each may assert the identity it acts as; the controller checks
//! it against the socket it arrived on.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{MachineAddReq, MachineRemoveReq, Request, StartReq};
use crate::secret::Secret;
use crate::user::{Identity, SecretName};

mod mcp;
pub use mcp::{McpAddReq, McpOAuthFinishReq, McpRemoveReq};

/// Arguments of a user request that needs only the identity it acts as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// Arguments of `secret-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
    /// The secret's value.
    pub value: SecretValue,
}

/// Arguments of `secret-delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDeleteReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
}

/// Arguments of `link-redeem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRedeemReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The code `link-code` minted.
    pub code: String,
}

/// Arguments of `proxy-release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyReleaseReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The token `proxy-lease` granted.
    pub token: SecretValue,
}

/// Arguments of `as`: another request, acting as the identity a front asserts for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActAsReq {
    /// The identity asserted, checked against the socket the request arrived on.
    pub identity: Identity,
    /// The request to run as that identity.
    pub request: Box<Request>,
}

/// Arguments of `start-needs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartNeedsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The repo, as `start` takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<crate::name::ForgeRepo>,
}

/// A secret value in a request. Its JSON form is the text itself, because `secret-set` must
/// carry it over the socket; `Debug` never shows it. Serialize a request only to send it.
pub struct SecretValue(Secret);

impl SecretValue {
    /// Wraps the value's bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Secret::new(bytes))
    }

    /// The value.
    #[must_use]
    pub const fn secret(&self) -> &Secret {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self::new(self.0.expose().to_vec())
    }
}

impl PartialEq for SecretValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose() == other.0.expose()
    }
}

impl Eq for SecretValue {}

impl Serialize for SecretValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = std::str::from_utf8(self.0.expose())
            .map_err(|_| serde::ser::Error::custom("a secret value must be UTF-8 text"))?;
        s.serialize_str(text)
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|text| Self::new(text.into_bytes()))
    }
}

/// A user request with its asserted identity taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCmd {
    /// The user and identities of the caller.
    Whoami,
    /// Create a user for the caller, or return the one it has.
    Enroll,
    /// Store one of the caller's user's secrets.
    SecretSet {
        /// The secret's name.
        name: SecretName,
        /// The secret's value.
        value: SecretValue,
    },
    /// Delete one of the caller's user's secrets.
    SecretDelete {
        /// The secret's name.
        name: SecretName,
    },
    /// The names of the caller's user's secrets.
    SecretNames,
    /// Add or replace one of the caller's user's MCP servers.
    McpAdd {
        /// The server's name.
        name: String,
        /// The server's URL.
        url: String,
        /// The header the key goes in.
        header: Option<String>,
        /// The key to store, if any.
        key: Option<SecretValue>,
    },
    /// Remove one of the caller's user's MCP servers and its key.
    McpRemove {
        /// The server's name.
        name: String,
    },
    /// The caller's user's MCP servers.
    McpList,
    /// Finish signing in to one of the caller's user's MCP servers.
    McpOAuthFinish {
        /// The `state` the sign-in started with.
        state: String,
        /// The authorization code.
        code: String,
        /// The issuer the authorization server named.
        iss: Option<String>,
    },
    /// Add or replace one of the caller's user's machines.
    MachineAdd {
        /// The machine's name.
        name: String,
        /// The ssh target.
        ssh: String,
        /// The machine's labels.
        labels: Vec<String>,
        /// How many Boxes may hold it at once.
        capacity: u16,
    },
    /// Remove one of the caller's user's machines.
    MachineRemove {
        /// The machine's name.
        name: String,
    },
    /// The caller's user's machines.
    MachineList,
    /// Mint a one-time code that links another identity to the caller's user.
    LinkCode,
    /// Link the caller to the user that minted `code`.
    LinkRedeem {
        /// The code.
        code: String,
    },
    /// Lease the front Proxy for the caller's user.
    ProxyLease,
    /// End the lease of `token`.
    ProxyRelease {
        /// The token `proxy-lease` granted.
        token: SecretValue,
    },
    /// Whether the front Proxy is running.
    ProxyStatus,
}

impl UserCmd {
    /// The command name.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Whoami => "whoami",
            Self::Enroll => "enroll",
            Self::SecretSet { .. } => "secret-set",
            Self::SecretDelete { .. } => "secret-delete",
            Self::SecretNames => "secret-names",
            Self::McpAdd { .. } => "mcp-add",
            Self::McpRemove { .. } => "mcp-remove",
            Self::McpList => "mcp-list",
            Self::McpOAuthFinish { .. } => "mcp-oauth-finish",
            Self::MachineAdd { .. } => "machine-add",
            Self::MachineRemove { .. } => "machine-remove",
            Self::MachineList => "machine-list",
            Self::LinkCode => "link-code",
            Self::LinkRedeem { .. } => "link-redeem",
            Self::ProxyLease => "proxy-lease",
            Self::ProxyRelease { .. } => "proxy-release",
            Self::ProxyStatus => "proxy-status",
        }
    }
}

impl Request {
    /// Splits a user request into the identity it asserts and its command; any other request
    /// comes back unchanged.
    ///
    /// # Errors
    /// The request itself, when it is not a user request.
    pub fn into_user(self) -> Result<(Option<Identity>, UserCmd), Box<Self>> {
        Ok(match self {
            Self::Whoami(r) => (r.identity, UserCmd::Whoami),
            Self::Enroll(r) => (r.identity, UserCmd::Enroll),
            Self::SecretSet(r) => (
                r.identity,
                UserCmd::SecretSet {
                    name: r.name,
                    value: r.value,
                },
            ),
            Self::SecretDelete(r) => (r.identity, UserCmd::SecretDelete { name: r.name }),
            Self::SecretNames(r) => (r.identity, UserCmd::SecretNames),
            Self::McpAdd(r) => (
                r.identity,
                UserCmd::McpAdd {
                    name: r.name,
                    url: r.url,
                    header: r.header,
                    key: r.key,
                },
            ),
            Self::McpRemove(r) => (r.identity, UserCmd::McpRemove { name: r.name }),
            Self::McpList(r) => (r.identity, UserCmd::McpList),
            Self::McpOAuthFinish(r) => (
                r.identity,
                UserCmd::McpOAuthFinish {
                    state: r.state,
                    code: r.code,
                    iss: r.iss,
                },
            ),
            Self::MachineAdd(r) => (
                r.identity,
                UserCmd::MachineAdd {
                    name: r.name,
                    ssh: r.ssh,
                    labels: r.labels,
                    capacity: r.capacity,
                },
            ),
            Self::MachineRemove(r) => (r.identity, UserCmd::MachineRemove { name: r.name }),
            Self::MachineList(r) => (r.identity, UserCmd::MachineList),
            Self::LinkCode(r) => (r.identity, UserCmd::LinkCode),
            Self::LinkRedeem(r) => (r.identity, UserCmd::LinkRedeem { code: r.code }),
            Self::ProxyLease(r) => (r.identity, UserCmd::ProxyLease),
            Self::ProxyRelease(r) => (r.identity, UserCmd::ProxyRelease { token: r.token }),
            Self::ProxyStatus(r) => (r.identity, UserCmd::ProxyStatus),
            other => return Err(Box::new(other)),
        })
    }

    /// Splits off the identity this request asserts: an `as` request's, else the request's
    /// own `identity` field.
    ///
    /// # Errors
    /// The refusal for an `as` around another `as`, or around a request that asserts a
    /// different identity of its own.
    pub fn split_identity(self) -> Result<(Option<Identity>, Self), String> {
        let Self::As(acting) = self else {
            return Ok((self.own_identity().cloned(), self));
        };
        let inner = *acting.request;
        if matches!(inner, Self::As(_)) {
            return Err("an as request cannot wrap another as request".to_owned());
        }
        match inner.own_identity() {
            Some(own) if *own != acting.identity => Err(format!(
                "the request asserts {own} inside as {}",
                acting.identity
            )),
            _ => Ok((Some(acting.identity), inner)),
        }
    }

    /// The identity this request's own field asserts.
    const fn own_identity(&self) -> Option<&Identity> {
        match self {
            Self::Start(StartReq { identity, .. })
            | Self::StartNeeds(StartNeedsReq { identity, .. })
            | Self::Whoami(AsReq { identity })
            | Self::Enroll(AsReq { identity })
            | Self::SecretNames(AsReq { identity })
            | Self::McpList(AsReq { identity })
            | Self::MachineList(AsReq { identity })
            | Self::MachineAdd(MachineAddReq { identity, .. })
            | Self::MachineRemove(MachineRemoveReq { identity, .. })
            | Self::McpAdd(McpAddReq { identity, .. })
            | Self::McpRemove(McpRemoveReq { identity, .. })
            | Self::McpOAuthFinish(McpOAuthFinishReq { identity, .. })
            | Self::LinkCode(AsReq { identity })
            | Self::ProxyLease(AsReq { identity })
            | Self::ProxyStatus(AsReq { identity })
            | Self::ProxyRelease(ProxyReleaseReq { identity, .. })
            | Self::SecretSet(SecretSetReq { identity, .. })
            | Self::SecretDelete(SecretDeleteReq { identity, .. })
            | Self::LinkRedeem(LinkRedeemReq { identity, .. }) => identity.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
