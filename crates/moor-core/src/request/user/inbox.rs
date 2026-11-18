//! Arguments of `inbox`.

use serde::{Deserialize, Serialize};

use crate::user::Identity;

/// What `inbox` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxOp {
    /// List the newest items, with their state.
    List,
    /// Mark items read.
    Read {
        /// The items; every item of the caller when empty.
        #[serde(default)]
        ids: Vec<i64>,
    },
}

/// Arguments of `inbox`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// What to do.
    pub op: InboxOp,
}
