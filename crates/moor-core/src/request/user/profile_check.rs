//! Arguments of `profile-check`.

use serde::{Deserialize, Serialize};

use crate::name::ProfileSource;
use crate::user::Identity;

/// Arguments of `profile-check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCheckReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The profile to check; the user's recorded one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProfileSource>,
}
