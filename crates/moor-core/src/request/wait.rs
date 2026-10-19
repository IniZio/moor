//! Requests that wait for a Box, or for events from several Boxes.

use serde::{Deserialize, Serialize};

use crate::name::BoxName;

/// Arguments of `wait-events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitEventsReq {
    /// Boxes to watch.
    pub boxes: Vec<BoxName>,
    /// Event kinds that count; any kind when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// Whether any or all Boxes must match.
    #[serde(default)]
    pub mode: WaitMode,
    /// Event cursor to start after; the newest when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// How `wait-events` combines Boxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitMode {
    /// One matching Box is enough.
    #[default]
    Any,
    /// Every Box must match.
    All,
}

/// Arguments of `wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitReq {
    /// Box name.
    pub task: BoxName,
    /// Condition to wait for.
    #[serde(rename = "for", default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<WaitFor>,
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Condition for `wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitFor {
    /// Agent idle.
    Idle,
    /// Task done.
    Done,
    /// Run finished.
    Run,
    /// Agent needs the owner's input, or is idle.
    Input,
}
