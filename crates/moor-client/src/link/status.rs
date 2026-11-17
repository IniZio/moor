//! What an agent says about itself when asked for its status: its release, platform, free
//! disk and the last ops it served. Read-only and bounded.

use serde::{Deserialize, Serialize};

/// Most ops a status lists.
pub const MAX_RECENT: usize = 50;
/// Most chars of an op's error a status quotes.
pub const MAX_ERROR: usize = 200;

/// An agent's answer to [`super::ops::Op::Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The agent's release.
    pub version: String,
    /// Its OS, as Rust names it.
    pub os: String,
    /// Its CPU architecture, as Rust names it.
    pub arch: String,
    /// Free space on the home's filesystem, in whole GB; none when it cannot be read.
    pub free_disk_gb: Option<u64>,
    /// The last [`MAX_RECENT`] ops it served, oldest first.
    pub recent: Vec<Served>,
}

/// One op the agent served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Served {
    /// The op's name on the wire.
    pub op: String,
    /// The Box id or lease slug it named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The first [`MAX_ERROR`] chars of why it failed; none when it was served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix seconds when it ended.
    pub at: i64,
}

/// The first `max` chars of `text`.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
