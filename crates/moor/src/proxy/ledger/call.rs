//! The store calls a Proxy makes. The shared egress Proxy names the Box in its hello, and each
//! call answers for that Box.

use moor_core::domain::event::Event;
use moor_core::user::SecretName;
use serde::{Deserialize, Serialize};

/// One store call.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Call {
    /// The Box as the shared egress Proxy sees it.
    View,
    /// What the Box holds, as the machine service reads it.
    Hold,
    /// Records a proxy audit event.
    RecordEvent {
        /// The event.
        event: Event,
    },
    /// The status of the open limit on credential rule `rule`.
    OpenLimit {
        /// The rule.
        rule: String,
    },
    /// The secret `name` of the Box's user.
    Secret {
        /// Which secret.
        name: SecretName,
    },
    /// The spec the Box's Proxy starts from.
    Spec,
    /// The Box's interception CA, made on first call.
    Ca,
    /// The token hash and Box of every open, awake Box that has an egress token.
    AwakeTokens,
    /// The open Box whose egress token hashes to `token_hash`.
    TokenBox {
        /// The hex SHA-256 of the token the Box presented.
        token_hash: String,
    },
    /// [`Call::TokenBox`], with a digest of the spec the Box's Proxy runs from. A controller
    /// from before this call refuses it.
    TokenSpec {
        /// The hex SHA-256 of the token the Box presented.
        token_hash: String,
    },
}

impl Call {
    /// Whether making the call twice leaves the store as making it once does, so a call that
    /// failed on a connection the controller may have closed can be made again.
    pub(crate) const fn repeatable(&self) -> bool {
        !matches!(self, Self::RecordEvent { .. })
    }
}
