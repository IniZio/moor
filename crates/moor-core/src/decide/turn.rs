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

/// A steer waiting for its turn to end, placed on the Box's hook clock so no controller time is
/// compared with a Box time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteerMark {
    /// The steer's event id.
    pub id: i64,
    /// When the newest hook report read before the send was written; none when there was none.
    pub hook_before: Option<Ts>,
    /// Whether the steer typed a prompt that starts a turn of its own; a menu choice or an
    /// interrupt acts on the turn already shown.
    pub new_turn: bool,
}

/// The last hook report for the Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookFacts {
    /// What it said.
    pub signal: HookSignal,
    /// When it was written.
    pub at: Ts,
    /// When the prompt of the turn it belongs to was submitted; none when the report names none.
    pub turn: Option<Ts>,
}

/// What the turn decider can observe.
#[derive(Debug, Clone, Copy)]
pub struct TurnFacts {
    /// The newest unanswered steer.
    pub steer: Option<SteerMark>,
    /// The last hook report for the Box.
    pub hook: Option<HookFacts>,
    /// Whether the agent's pane has exited.
    pub pane_exited: bool,
}

/// The steer's event id and outcome once the turn it belongs to is done.
///
/// An exited pane wins. Otherwise an Idle or Blocked report counts when it is past the
/// last report read before the send: for a typed prompt, its turn was submitted after that report,
/// so the end of a turn already running does not count; for a menu choice or an interrupt, it was
/// written after that report.
#[must_use]
pub fn turn_done(f: &TurnFacts) -> Option<(i64, TurnOutcome)> {
    let steer = f.steer?;
    if f.pane_exited {
        return Some((steer.id, TurnOutcome::Exited));
    }
    let hook = f.hook?;
    let seen = if steer.new_turn { hook.turn? } else { hook.at };
    if steer.hook_before.is_some_and(|before| seen <= before) {
        return None;
    }
    match hook.signal {
        HookSignal::Idle => Some((steer.id, TurnOutcome::Idle)),
        HookSignal::Blocked => Some((steer.id, TurnOutcome::NeedsInput)),
        HookSignal::Other => None,
    }
}

#[cfg(test)]
mod tests;
