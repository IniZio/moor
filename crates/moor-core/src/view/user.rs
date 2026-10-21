//! Typed `value` payloads for the user requests; none carries a secret value.

use serde::{Deserialize, Serialize};

use crate::user::{Identity, SecretName, UserId};

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
