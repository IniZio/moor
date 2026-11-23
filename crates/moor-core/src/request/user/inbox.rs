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
    /// Take items out of the list. Their rows stay, so an event written again cannot bring
    /// one back, and the task each is about is unchanged.
    Clear(InboxClear),
}

/// Which items a clear takes out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxClear {
    /// These items; none when empty.
    Ids(Vec<i64>),
    /// Every item up to this id, the newest the person saw, so one that lands later stays.
    Through(i64),
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
