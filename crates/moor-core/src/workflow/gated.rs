//! Where a run of a Box with gates goes next: the gates say whether it is done, failed, held for a
//! person or still being worked, and the run follows them. The agent going quiet decides nothing.

use super::StepId;
use crate::gate::{GateNext, GateStatus};

/// What the tick knows of a run of a Box with gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing<'a> {
    /// The step and try the run is at.
    pub at: (&'a StepId, u8),
    /// Whether the run waits for a person, and not works.
    pub waiting: bool,
    /// Whether its Box can still be steered.
    pub box_open: bool,
    /// Whether its agent is quiet enough for moor to act on it.
    pub quiet: bool,
}

/// The move a run makes from where its gates stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatedMove {
    /// Nothing changes.
    Stay,
    /// The run works on the gate in hand, which is not the step it is at.
    Enter {
        /// The gate.
        gate: StepId,
        /// Its try.
        try_n: u8,
    },
    /// The run waits for a person at the gate.
    Wait {
        /// The gate.
        gate: StepId,
        /// Its try.
        try_n: u8,
    },
    /// The wait is over, and the run works on the gate again.
    Resume {
        /// The gate.
        gate: StepId,
        /// Its try.
        try_n: u8,
    },
    /// Every gate passed.
    Deliver,
    /// A gate failed for good.
    Fail {
        /// The gate.
        gate: StepId,
        /// Why.
        reason: String,
    },
    /// The Box is gone or closed with gates not passed, so nobody is left to pass them.
    Incomplete,
}

/// The try a wait at a stuck gate is recorded at when the run is not at that gate: the gate has
/// no try in hand then. A run at the gate keeps its try.
const STUCK_TRY: u8 = 1;

/// The move for a run that stands as `now` says, when its gates stand at `status`.
#[must_use]
pub fn gated_move(now: Standing<'_>, status: &GateStatus) -> GatedMove {
    match status {
        GateStatus::Passed => GatedMove::Deliver,
        GateStatus::Failed { gate, why } => GatedMove::Fail {
            gate: gate.clone(),
            reason: why.clone(),
        },
        _ if !now.box_open => GatedMove::Incomplete,
        GateStatus::Stuck { gate, .. } if !now.waiting => GatedMove::Wait {
            gate: gate.clone(),
            try_n: if now.at.0 == gate {
                now.at.1
            } else {
                STUCK_TRY
            },
        },
        GateStatus::Open {
            gate,
            try_n,
            next: GateNext::Ask,
        } if !now.waiting && now.quiet => GatedMove::Wait {
            gate: gate.clone(),
            try_n: *try_n,
        },
        GateStatus::Open {
            gate,
            try_n,
            next: GateNext::Agent | GateNext::RunChecks,
        } if now.waiting => GatedMove::Resume {
            gate: gate.clone(),
            try_n: *try_n,
        },
        GateStatus::Open {
            gate,
            try_n,
            next: GateNext::Agent | GateNext::RunChecks,
        } if now.at != (gate, *try_n) => GatedMove::Enter {
            gate: gate.clone(),
            try_n: *try_n,
        },
        GateStatus::Open { .. } | GateStatus::Stuck { .. } => GatedMove::Stay,
    }
}

#[cfg(test)]
mod tests;
