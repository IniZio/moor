//! Verdict on whether the turn a steer message started is done.

use crate::domain::event::TurnOutcome;

/// What the agent's terminal says about its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSignal {
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    Blocked,
    /// Anything else: the agent is still working, or has not reported.
    Other,
}

/// A steer waiting for its turn to end, counted in the terminal's ended turns so no controller
/// time is compared with a Box time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteerMark {
    /// The steer's event id.
    pub id: i64,
    /// The ended-turn count the steer's turn must pass; none when the send could not tell.
    pub turn_before: Option<u64>,
}

/// The agent's terminal now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermTurn {
    /// What its status says.
    pub signal: TurnSignal,
    /// How many turns have ended.
    pub turn: u64,
}

/// What the turn decider can observe.
#[derive(Debug, Clone, Copy)]
pub struct TurnFacts {
    /// The newest unanswered steer.
    pub steer: Option<SteerMark>,
    /// The agent's terminal, when read.
    pub term: Option<TermTurn>,
    /// Whether the agent has exited.
    pub exited: bool,
}

/// The steer's event id and outcome once the turn it belongs to is done.
///
/// An exit wins. Otherwise an idle or blocked terminal counts once its ended turns are past the
/// steer's mark.
#[must_use]
pub fn turn_done(f: &TurnFacts) -> Option<(i64, TurnOutcome)> {
    let steer = f.steer?;
    if f.exited {
        return Some((steer.id, TurnOutcome::Exited));
    }
    let term = f.term?;
    if steer.turn_before.is_some_and(|before| term.turn <= before) {
        return None;
    }
    match term.signal {
        TurnSignal::Idle => Some((steer.id, TurnOutcome::Idle)),
        TurnSignal::Blocked => Some((steer.id, TurnOutcome::NeedsInput)),
        TurnSignal::Other => None,
    }
}

#[cfg(test)]
mod tests;
