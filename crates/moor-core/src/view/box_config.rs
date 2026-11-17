//! Typed `value` payload of `box-config-set`.

use serde::{Deserialize, Serialize};

/// A Box's config override after a `box-config-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxConfigValue {
    /// The Box.
    #[serde(rename = "box")]
    pub box_name: String,
    /// Whether the Box now has an override; false after one was removed.
    pub set: bool,
}
