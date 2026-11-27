//! Events about where a Box's traffic went: an intercepted TLS handshake cut off or back, and
//! the hosts the Box reached.

use crate::domain::egress::EgressRow;

/// A host the Box's Proxy intercepts, and the tool rule it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EgressHost {
    /// The host the connection was for.
    pub host: String,
    /// The tool rule the host belongs to, when it belongs to one.
    pub rule: Option<String>,
}

/// Where a Box's traffic went.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EgressEvent {
    /// A client in the Box hung up during an intercepted TLS handshake without sending an alert; the Proxy refused nothing.
    EgressAborted(EgressHost),
    /// The Proxy allowed a request to a host whose intercepted TLS handshake was aborted earlier.
    EgressRecovered(EgressHost),
    /// The Proxy's push guard refused a push: it moved a ref the Box does not own, deleted a ref,
    /// or could not be read.
    PushRefused {
        /// The git host the push was for.
        host: String,
        /// The repo path the push was for, such as `acme/widgets`.
        repo: String,
        /// The ref the refusal names, such as `refs/heads/main`; none for a push moor could not read.
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        /// The contract key the refusal keeps, such as `branch`.
        key: String,
        /// What the push was told, the ref and the key included.
        reason: String,
    },
    /// The Box reached hosts it had not reached before, or a count changed.
    EgressSeen {
        /// The rows that are new or whose count changed.
        hosts: Vec<EgressRow>,
    },
}

impl EgressEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::EgressAborted(_) => "egress_aborted",
            Self::EgressRecovered(_) => "egress_recovered",
            Self::EgressSeen { .. } => "egress_seen",
            Self::PushRefused { .. } => "push_refused",
        }
    }
}
