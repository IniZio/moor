//! The host an intercepted TLS handshake was for.

/// A host the Box's Proxy intercepts, and the tool rule it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EgressHost {
    /// The host the connection was for.
    pub host: String,
    /// The tool rule the host belongs to, when it belongs to one.
    pub rule: Option<String>,
}
