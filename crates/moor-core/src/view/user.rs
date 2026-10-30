//! Typed `value` payloads for the user requests; none carries a stored secret's value.

use serde::{Deserialize, Serialize};

use crate::name::ProfileSource;
use crate::request::SecretValue;
use crate::user::{Email, Identity, SecretName, UserId};

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
    /// When the server was signed in to over OAuth: when its access token expires, in Unix
    /// milliseconds, or `None` when the server named no expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<Option<i64>>,
    /// Whether the server's sign-in ended and the user must sign in again with `mcp-add`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sign_in_again: bool,
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
    /// The address to open to sign in, when the server asked for OAuth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<String>,
}

/// Value of `profile-set`, `profile-show` and `profile-remove`: the profile repo now recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileValue {
    /// The repo and ref, or `None` when none is set.
    pub source: Option<ProfileSource>,
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
    /// The crabbox provider it offers Boxes on, when it hosts a Box pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// Its agent's link: online or not, since when, and the agent's release. None until its
    /// agent first links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<crate::request::MachineLink>,
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

/// Value of `machine-keys`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineKeysValue {
    /// Every enrolled machine, by id.
    pub machines: Vec<MachineKey>,
}

/// An enrolled machine and the key its agent signs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineKey {
    /// The machine's id.
    pub machine_id: i64,
    /// The user that owns it.
    pub user: UserId,
    /// The machine's name.
    pub name: String,
    /// The agent's ed25519 public key as an OpenSSH line.
    pub pubkey: String,
}

/// Value of `machine-links`: what the store now holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineLinksValue {
    /// The link of every enrolled machine the runner reported, by id.
    pub links: Vec<crate::request::MachineLink>,
}

/// Value of `machine-add` and `machine-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineChangeValue {
    /// The machine's name.
    pub name: String,
    /// Whether anything changed.
    pub changed: bool,
}

/// Value of `machine-enroll`: what the agent keeps in its state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEnrollValue {
    /// The machine's id, kept when the same name enrolls again.
    pub machine_id: i64,
    /// The machine's name.
    pub name: String,
    /// The user that owns it.
    pub user: UserId,
    /// The verified email the user was found by; none for an enrollment without a front.
    pub owner: Option<Email>,
    /// How many Boxes may hold it at once.
    pub capacity: u16,
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
