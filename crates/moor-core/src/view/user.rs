//! Typed `value` payloads for the user requests.

use serde::{Deserialize, Serialize};

use crate::user::{Identity, UserId};

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
