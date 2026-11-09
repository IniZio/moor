//! What a Box's queued inbox rows wait for, and what moor does next to deliver them.

use crate::domain::box_state::BoxLifecycle;
use crate::domain::hands::HandsState;
use crate::domain::inbox::QueueCause;

/// What a read of the agent showed, for delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFact {
    /// A live launch whose status reads Idle or Working.
    Ready,
    /// A live launch that cannot take a message now, or one not read.
    NotReady,
    /// The agent ended, and moor may start it again.
    Dead,
}

/// Where a Box stands, as delivery needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// The Box's lifecycle.
    pub lifecycle: BoxLifecycle,
    /// Its machine's state; none before one is acquired.
    pub hands: Option<HandsState>,
    /// What the agent showed; only read on a Box that is awake.
    pub agent: AgentFact,
    /// Whether moor may not relaunch the agent on its own now.
    pub capped: bool,
}

/// What moor does next for a Box with queued rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Send the rows to the agent.
    Deliver,
    /// Wake the Box, which delivers at its end.
    Wake,
    /// Start the Box on new hands, which delivers at its end.
    BringBack,
    /// Start the agent again, then deliver.
    Revive,
    /// Nothing now; a later pass or op delivers.
    Hold,
}

/// What moor does next for a Box at `f` that has queued rows.
#[must_use]
pub const fn next(f: &Facts) -> Next {
    match (f.hands, f.lifecycle) {
        // A failed setup waits for a person to retry or close it, its rows kept for that retry.
        (_, BoxLifecycle::SetupFailed) => Next::Hold,
        (Some(HandsState::Gone), _) => Next::BringBack,
        (_, BoxLifecycle::Asleep) => Next::Wake,
        (Some(HandsState::Ready), BoxLifecycle::Ready) => match f.agent {
            AgentFact::Ready => Next::Deliver,
            AgentFact::Dead if !f.capped => Next::Revive,
            AgentFact::Dead | AgentFact::NotReady => Next::Hold,
        },
        _ => Next::Hold,
    }
}

/// Why a row of a Box at `f` waits.
#[must_use]
pub const fn cause(f: &Facts) -> QueueCause {
    match (f.hands, f.lifecycle) {
        (Some(HandsState::Gone), _) => QueueCause::Relaunching,
        (_, BoxLifecycle::Asleep) => QueueCause::Asleep,
        (Some(HandsState::Asleep), BoxLifecycle::Pending) => QueueCause::WaitingForRoom,
        (_, BoxLifecycle::Ready) if matches!(f.agent, AgentFact::Dead) => {
            if f.capped {
                QueueCause::RelaunchCapped
            } else {
                QueueCause::Relaunching
            }
        }
        _ => QueueCause::Busy,
    }
}

#[cfg(test)]
mod tests;
