//! Arguments of the `node-settings-*` requests: a node's cap and labels, kept in the store.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::user::Identity;

/// Arguments of `node-settings-get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSettingsGetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The node's name.
    pub name: String,
}

/// What a write does to a node's `max_boxes`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapEdit {
    /// Leave the cap as it is.
    #[default]
    Keep,
    /// Remove the cap, so free memory alone decides.
    Clear,
    /// Hold at most this many live Boxes.
    Set(NonZeroU32),
}

impl CapEdit {
    /// Whether it leaves the cap alone, which the wire leaves out.
    #[must_use]
    pub const fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

/// Arguments of `node-settings-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSettingsSetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The node's name.
    pub name: String,
    /// What to do with the cap.
    #[serde(default, skip_serializing_if = "CapEdit::is_keep")]
    pub max_boxes: CapEdit,
    /// The node's whole label list; absent leaves the labels as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// The revision the writer last saw, 0 for a node never written; the write is refused when
    /// the stored revision differs. Absent writes over whatever is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

#[cfg(test)]
mod tests;
