//! Requests about moor users. Each may assert the identity it acts as; the controller checks
//! it against the socket it arrived on.

use serde::{Deserialize, Serialize};

use super::{MachineEnrollReq, MachineRemoveReq, MachineStatusReq, Request, StartReq};
use crate::name::ProfileSource;
use crate::user::{Email, Identity, SecretName};

mod mcp;
mod secret_value;
pub use mcp::{McpAddReq, McpOAuthFinishReq, McpRemoveReq, ProfileSetReq};
pub use secret_value::SecretValue;

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

/// Arguments of `enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The email the front verified for that identity; only a front socket may send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,
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

/// A user request with its asserted identity taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCmd {
    /// The user and identities of the caller.
    Whoami,
    /// Create a user for the caller, or return the one it has.
    Enroll {
        /// The email the front verified for the caller.
        email: Option<Email>,
    },
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
    /// Record the caller's user's agent profile repo, replacing any earlier one.
    ProfileSet {
        /// The repo, and the ref to read it at.
        source: ProfileSource,
    },
    /// The caller's user's agent profile repo.
    ProfileShow,
    /// Clear the caller's user's agent profile repo.
    ProfileRemove,
    /// Finish signing in to one of the caller's user's MCP servers.
    McpOAuthFinish {
        /// The `state` the sign-in started with.
        state: String,
        /// The authorization code.
        code: String,
        /// The issuer the authorization server named.
        iss: Option<String>,
    },
    /// Enroll the caller's user, then add or replace a machine its agent enrolls.
    MachineEnroll {
        /// The email the front verified.
        email: Option<Email>,
        /// The machine's name.
        name: String,
        /// The agent's public key.
        pubkey: String,
        /// The machine's labels.
        labels: Vec<String>,
        /// How many Boxes may hold it at once.
        capacity: u16,
        /// The crabbox provider the machine offers Boxes on.
        pool: Option<String>,
    },
    /// Remove one of the caller's user's machines.
    MachineRemove {
        /// The machine's name.
        name: String,
    },
    /// The caller's user's machines.
    MachineList,
    /// Ask one of the caller's user's machine agents for its status.
    MachineStatus {
        /// The machine's name.
        name: String,
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
            Self::Enroll { .. } => "enroll",
            Self::SecretSet { .. } => "secret-set",
            Self::SecretDelete { .. } => "secret-delete",
            Self::SecretNames => "secret-names",
            Self::McpAdd { .. } => "mcp-add",
            Self::McpRemove { .. } => "mcp-remove",
            Self::McpList => "mcp-list",
            Self::McpOAuthFinish { .. } => "mcp-oauth-finish",
            Self::ProfileSet { .. } => "profile-set",
            Self::ProfileShow => "profile-show",
            Self::ProfileRemove => "profile-remove",
            Self::MachineEnroll { .. } => "machine-enroll",
            Self::MachineRemove { .. } => "machine-remove",
            Self::MachineList => "machine-list",
            Self::MachineStatus { .. } => "machine-status",
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
            Self::Enroll(r) => (r.identity, UserCmd::Enroll { email: r.email }),
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
            Self::ProfileSet(r) => (r.identity, UserCmd::ProfileSet { source: r.source }),
            Self::ProfileShow(r) => (r.identity, UserCmd::ProfileShow),
            Self::ProfileRemove(r) => (r.identity, UserCmd::ProfileRemove),
            Self::McpOAuthFinish(r) => (
                r.identity,
                UserCmd::McpOAuthFinish {
                    state: r.state,
                    code: r.code,
                    iss: r.iss,
                },
            ),
            Self::MachineEnroll(r) => (
                r.identity,
                UserCmd::MachineEnroll {
                    email: r.email,
                    name: r.name,
                    pubkey: r.pubkey,
                    labels: r.labels,
                    capacity: r.capacity,
                    pool: r.pool,
                },
            ),
            Self::MachineRemove(r) => (r.identity, UserCmd::MachineRemove { name: r.name }),
            Self::MachineList(r) => (r.identity, UserCmd::MachineList),
            Self::MachineStatus(r) => (r.identity, UserCmd::MachineStatus { name: r.name }),
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
            | Self::Enroll(EnrollReq { identity, .. })
            | Self::SecretNames(AsReq { identity })
            | Self::McpList(AsReq { identity })
            | Self::ProfileShow(AsReq { identity })
            | Self::ProfileRemove(AsReq { identity })
            | Self::ProfileSet(ProfileSetReq { identity, .. })
            | Self::MachineList(AsReq { identity })
            | Self::NodeList(AsReq { identity })
            | Self::MachineRemove(MachineRemoveReq { identity, .. })
            | Self::MachineStatus(MachineStatusReq { identity, .. })
            | Self::MachineEnroll(MachineEnrollReq { identity, .. })
            | Self::McpAdd(McpAddReq { identity, .. })
            | Self::McpRemove(McpRemoveReq { identity, .. })
            | Self::McpOAuthFinish(McpOAuthFinishReq { identity, .. })
            | Self::ProxyLease(AsReq { identity })
            | Self::ProxyStatus(AsReq { identity })
            | Self::ProxyRelease(ProxyReleaseReq { identity, .. })
            | Self::SecretSet(SecretSetReq { identity, .. })
            | Self::SecretDelete(SecretDeleteReq { identity, .. }) => identity.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
