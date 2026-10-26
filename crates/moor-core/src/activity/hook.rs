//! The neutral hook report an agent profile parses its hook file into.

use crate::seam::clock::Ts;

/// What a hook report says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookKind {
    /// The session started; no prompt yet.
    Starting {
        /// The launch that wrote this marker; `None` when the marker names none.
        launch: Option<String>,
    },
    /// A prompt is being worked on.
    Busy,
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    Blocked,
}

/// One hook report, neutral across agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRecord {
    /// What it reports.
    pub kind: HookKind,
    /// The agent session it came from.
    pub session_id: Option<String>,
    /// The text the agent attached: the question it waits on, or its last message when idle.
    pub message: Option<String>,
    /// The choices the agent offered with its question, in order; empty when it named none.
    pub options: Vec<String>,
    /// What the agent is doing now, one short line; set only on a busy report.
    pub doing: Option<String>,
    /// When the agent wrote it.
    pub at: Ts,
    /// When the prompt of the turn it belongs to was submitted; none when the report names none.
    pub turn: Option<Ts>,
}
