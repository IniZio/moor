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
        }
    }
}
