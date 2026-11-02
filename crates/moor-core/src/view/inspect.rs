//! Typed `value` payload of `inspect`.

use super::{AgentView, PaneView, SaidView};

/// Why a Box looks stuck, from the newest unanswered stuck event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StuckView {
    /// `stalled`, `egress_refused`, `egress_aborted`, `credential_withheld`,
    /// `proxy_credential_failed`, `agent_rate_limited`, `agent_unreachable` or
    /// `agent_link_lost`.
    pub kind: String,
    /// RFC3339 UTC time of the event.
    pub since: String,
    /// One line saying what happened.
    pub detail: String,
}

/// One Box looked at for `inspect`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InspectView {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
    /// Lifecycle state.
    pub lifecycle: String,
    /// Session state.
    pub session: String,
    /// Agent activity.
    pub agent: Option<AgentView>,
    /// The tail of the agent's pane.
    pub pane: Option<PaneView>,
    /// The agent's latest messages, oldest first.
    pub said: Vec<SaidView>,
    /// Why the Box looks stuck, when it does.
    pub stuck: Option<StuckView>,
}
