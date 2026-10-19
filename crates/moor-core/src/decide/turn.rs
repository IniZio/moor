//! Verdict on whether the turn a steer message started is done.

use crate::activity::{HookKind, HookRecord};
use crate::seam::clock::Ts;

/// How a finished turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    NeedsInput,
    /// The agent's pane is gone.
    Exited,
}

impl TurnOutcome {
    /// The wire name of the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::NeedsInput => "needs_input",
            Self::Exited => "exited",
        }
    }
}

/// What the turn decider can observe.
#[derive(Debug, Clone, Copy)]
pub struct TurnFacts<'a> {
    /// The newest unanswered steer: its event id and when it was sent.
    pub steer: Option<(i64, Ts)>,
    /// The last hook report for the Task.
    pub hook: Option<&'a HookRecord>,
    /// Whether the agent's pane has exited.
    pub pane_exited: bool,
}

/// Decides whether the turn started by the steer is done. Returns the steer's
/// event id with the outcome. An exited pane wins; otherwise only an Idle or
/// Blocked report written after the steer counts.
#[must_use]
pub fn turn_done(f: &TurnFacts<'_>) -> Option<(i64, TurnOutcome)> {
    let (id, sent) = f.steer?;
    if f.pane_exited {
        return Some((id, TurnOutcome::Exited));
    }
    let hook = f.hook.filter(|h| h.at > sent)?;
    match hook.kind {
        HookKind::Idle => Some((id, TurnOutcome::Idle)),
        HookKind::Blocked => Some((id, TurnOutcome::NeedsInput)),
        HookKind::Starting | HookKind::Busy => None,
    }
}

#[cfg(test)]
mod tests;
