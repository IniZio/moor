//! Typed `value` payloads for the user requests; none carries a stored secret's value.

use serde::{Deserialize, Serialize};

use crate::request::SecretValue;
use crate::user::{Identity, SecretName, UserId};

/// One of a user's MCP servers as `mcp-list` shows it; never its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerView {
    /// The server's name.
    pub name: String,
    /// Its URL.
    pub url: String,
    /// The header its key goes in.
    pub header: String,
    /// The vault secret that holds its key.
    pub secret: SecretName,
    /// Whether that secret is stored.
    pub has_key: bool,
}

/// Value of `mcp-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpListValue {
    /// The servers by name.
    pub servers: Vec<McpServerView>,
}

/// Value of `mcp-add` and `mcp-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpChangeValue {
    /// The server's name.
    pub name: String,
    /// Whether anything changed.
    pub changed: bool,
    /// The vault secret that holds its key.
    pub secret: SecretName,
}

/// One of a user's machines as `machine-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineView {
    /// The machine's name.
    pub name: String,
    /// Its ssh target.
    pub ssh: String,
    /// Its labels, sorted.
    pub labels: Vec<String>,
    /// How many Boxes may hold it at once.
    pub capacity: u16,
}

/// Value of `machine-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineListValue {
    /// The machines by name.
    pub machines: Vec<MachineView>,
}

/// Value of `machine-targets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineTargetsValue {
    /// The distinct ssh targets of every user's machines, sorted.
    pub targets: Vec<String>,
}

/// Value of `machine-add` and `machine-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineChangeValue {
    /// The machine's name.
    pub name: String,
    /// Whether anything changed.
    pub changed: bool,
}

/// Value of `whoami`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoamiValue {
    /// The identity the request acted as.
    pub identity: Identity,
    /// Its user; none until it enrolls or links.
    pub user: Option<UserId>,
    /// Every identity linked to that user, in order.
    pub identities: Vec<Identity>,
}

/// Value of `enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollValue {
    /// The caller's user.
    pub user: UserId,
    /// True when this call created it.
    pub created: bool,
}

/// Value of `secret-set` and `secret-delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretChangeValue {
    /// The secret's name; never its value.
    pub name: SecretName,
    /// True when the call changed a stored secret: set always does, delete only when it was there.
    pub changed: bool,
}

/// Value of `secret-names`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretNamesValue {
    /// The names of the user's secrets, in order.
    pub names: Vec<SecretName>,
}

/// Value of `link-code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCodeValue {
    /// The one-time code; another identity redeems it with `link-redeem`.
    pub code: String,
    /// Seconds until it expires.
    pub expires_in_secs: u64,
}

/// Value of `link-redeem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedValue {
    /// The user the caller is now linked to.
    pub user: UserId,
    /// The identity that was linked.
    pub identity: Identity,
}

/// Value of `start-needs`, and of a `start` refused for missing secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartNeedsValue {
    /// The identity the request acted as.
    pub identity: Identity,
    /// Its user; none until it enrolls or links.
    pub user: Option<UserId>,
    /// Every secret the Box's Proxy rules inject, in order.
    pub needs: Vec<SecretName>,
    /// The ones the user lacks, all of them at once, in order.
    pub missing: Vec<SecretName>,
}

/// Value of `proxy-lease`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "lease", rename_all = "snake_case")]
pub enum ProxyLeaseValue {
    /// The lease: requests that present `token` as the proxy password carry the user's
    /// secrets until it is released or expires.
    Granted {
        /// The lease token; it stands in for no secret, but it lets its holder spend them.
        token: SecretValue,
        /// Seconds until the lease expires.
        expires_in_secs: u64,
    },
    /// The user lacks the secret `name`, which a rule of the front's Proxy injects.
    NoSecret {
        /// The missing secret's name.
        name: SecretName,
    },
    /// The front Proxy is not running, so no lease was granted.
    ProxyDown,
}

/// Value of `proxy-release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyReleaseValue {
    /// True when the call ended a live lease.
    pub released: bool,
}

/// Value of `proxy-status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyStatusValue {
    /// True when the front's Proxy accepts connections.
    pub running: bool,
}
