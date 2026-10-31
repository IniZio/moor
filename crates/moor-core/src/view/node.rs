//! The `node-list` value.

use serde::{Deserialize, Serialize};

use crate::user::UserId;

/// Who may use a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeScope {
    /// Every user.
    Shared,
    /// Its owner only.
    Personal,
}

impl NodeScope {
    /// The lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Personal => "personal",
        }
    }
}

/// One node as `node-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeView {
    /// The node's name.
    pub name: String,
    /// Who may use it.
    pub scope: NodeScope,
    /// The owning user of a personal node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
    /// The runtime its Boxes run on.
    pub runtime: String,
    /// Whether its agent link is up; a shared node always is.
    pub online: bool,
    /// When an offline node went down, unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<i64>,
    /// Boxes holding a slot on it.
    pub used: usize,
    /// Boxes pending on it.
    pub waiting: usize,
    /// Most Boxes it hosts; none means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_boxes: Option<usize>,
    /// Its labels.
    pub labels: Vec<String>,
}

/// Value of `node-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeListValue {
    /// The nodes the caller sees, shared first.
    pub nodes: Vec<NodeView>,
}
