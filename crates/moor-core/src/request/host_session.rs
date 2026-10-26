//! Arguments of the host Session requests.

use serde::{Deserialize, Serialize};

use crate::name::SessionId;

/// Arguments of `open-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenHostSessionReq {
    /// Absolute directory the agent runs in on the host. Only the operator chooses one; for
    /// anyone else the controller gives the Session a directory of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Agent to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Stored agent session to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// Parent Session of the host Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// The caller's own name for the Session, opaque to the controller. While the caller has
    /// a live host Session with this key, opening again returns that Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Arguments of `update-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateHostSessionReq {
    /// Session id.
    pub session: SessionId,
    /// Agent to record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Stored agent session to record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

/// Arguments of `end-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndHostSessionReq {
    /// Session id.
    pub session: SessionId,
}
