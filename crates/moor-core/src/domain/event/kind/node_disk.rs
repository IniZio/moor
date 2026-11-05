//! Events about the host disk of the node a Box is on.

use serde::Serialize;

/// The node's disk ran low, or came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeDiskEvent {
    /// The node's free disk fell below what it keeps for another Box.
    NodeDiskLow {
        /// The node.
        node: String,
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
    },
    /// The node's free disk climbed back past its floor and margin.
    NodeDiskRecovered {
        /// The node.
        node: String,
        /// Free MiB.
        free_mib: u64,
        /// The filesystem's size in MiB.
        total_mib: u64,
    },
}

impl NodeDiskEvent {
    /// The event's kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NodeDiskLow { .. } => "node_disk_low",
            Self::NodeDiskRecovered { .. } => "node_disk_recovered",
        }
    }
}
