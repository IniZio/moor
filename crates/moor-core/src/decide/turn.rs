//! Verdict on whether the turn a steer message started is done.

use crate::domain::event::TurnOutcome;
use crate::seam::clock::Ts;

/// What a hook report said about the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSignal {
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    Blocked,
    /// Any other report: the agent is still working.
    Other,
}

/// What the turn decider can observe.
#[derive(Debug, Clone, Copy)]
pub struct TurnFacts {
    /// The newest unanswered steer: its event id and when it was sent.
    pub steer: Option<(i64, Ts)>,
    /// The last hook report for the Task and when it was written.
    pub hook: Option<(HookSignal, Ts)>,
    /// Whether the agent's pane has exited.
    pub pane_exited: bool,
}

/// Decides whether the turn started by the steer is done. Returns the steer's
/// event id with the outcome. An exited pane wins; otherwise only an Idle or
/// Blocked report written after the steer counts.
#[must_use]
pub fn turn_done(f: &TurnFacts) -> Option<(i64, TurnOutcome)> {
    let (id, sent) = f.steer?;
    if f.pane_exited {
        return Some((id, TurnOutcome::Exited));
    }
    let (signal, at) = f.hook?;
    if at <= sent {
        return None;
    }
    match signal {
        HookSignal::Idle => Some((id, TurnOutcome::Idle)),
        HookSignal::Blocked => Some((id, TurnOutcome::NeedsInput)),
        HookSignal::Other => None,
    }
}

#[cfg(test)]
mod tests;
