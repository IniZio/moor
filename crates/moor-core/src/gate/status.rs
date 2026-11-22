//! Where a Box stands against its gates and outcomes: a pure fold over the lines written for them.

use super::fold::walk;
use super::{Gate, GateLine, Gates, Outcome, Verdict};
use crate::workflow::StepId;

/// What an open gate waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateNext {
    /// The agent's report and its evidence.
    Agent,
    /// moor running the gate's checks.
    RunChecks,
    /// The owner's answer.
    Ask,
    /// moor running the outcomes' checks at the closing stage.
    Decide,
}

/// Where a Box stands against its gates.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GateStatus {
    /// A gate is being tried.
    Open {
        /// The gate.
        gate: StepId,
        /// The try, from 1.
        try_n: u8,
        /// What it waits for.
        next: GateNext,
    },
    /// A gate waits for a person: the agent went quiet, the tries are used up, or moor could not
    /// run a check twice in a row.
    Stuck {
        /// The gate.
        gate: StepId,
        /// The try it was at when it got stuck.
        try_n: u8,
        /// Why.
        why: String,
    },
    /// A gate failed for good.
    Failed {
        /// The gate.
        gate: StepId,
        /// Why.
        why: String,
    },
    /// The run reached an outcome: every gate passed, or an `anytime` outcome held first.
    Passed {
        /// The outcome.
        outcome: Verdict,
    },
}

impl GateStatus {
    /// Whether the gates can still move: one is open or stuck, rather than failed for good or
    /// passed.
    #[must_use]
    pub const fn can_move(&self) -> bool {
        matches!(self, Self::Open { .. } | Self::Stuck { .. })
    }
}

/// Where a Box stands, from its `gates` and the `lines` written for them, oldest first.
#[must_use]
pub fn status(gates: &Gates, lines: &[GateLine]) -> GateStatus {
    walk(gates, lines).status()
}

/// The check moor runs next: the gate it belongs to, the try, and its place among the gate's checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueCheck<'a> {
    /// The gate.
    pub gate: &'a Gate,
    /// The try, from 1.
    pub try_n: u8,
    /// The check's place among the gate's checks, from 0.
    pub check: u8,
}

/// The check due from `gates` and the `lines` written for them; none unless the status is open at
/// `RunChecks`.
#[must_use]
pub fn due_check<'a>(gates: &'a Gates, lines: &[GateLine]) -> Option<DueCheck<'a>> {
    let walk = walk(gates, lines);
    let GateStatus::Open {
        try_n,
        next: GateNext::RunChecks,
        ..
    } = walk.status()
    else {
        return None;
    };
    Some(DueCheck {
        gate: gates.get(walk.at)?,
        try_n,
        check: walk.tries.checks,
    })
}

/// The outcomes moor tries next, in order, at the stage and try in hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueOutcomes<'a> {
    /// The gate in hand, or the closing stage.
    pub stage: StepId,
    /// The try, from 1.
    pub try_n: u8,
    /// What to try: the `anytime` outcomes at a gate, every outcome at the closing stage.
    pub outcomes: Vec<&'a Outcome>,
}

/// The outcomes due from `gates` and the `lines` written for them.
///
/// Every outcome when the status is open at `Decide`, and the `anytime` ones, if any, when a gate
/// waits for the agent or its checks, which moor tries once the agent is quiet, before it reminds
/// it or runs the check. None otherwise.
#[must_use]
pub fn due_outcomes<'a>(gates: &'a Gates, lines: &[GateLine]) -> Option<DueOutcomes<'a>> {
    let GateStatus::Open { gate, try_n, next } = status(gates, lines) else {
        return None;
    };
    let outcomes: Vec<&Outcome> = match next {
        GateNext::Decide => gates.outcomes().iter().collect(),
        GateNext::Agent | GateNext::RunChecks => {
            gates.outcomes().iter().filter(|o| o.anytime).collect()
        }
        GateNext::Ask => return None,
    };
    (!outcomes.is_empty()).then_some(DueOutcomes {
        stage: gate,
        try_n,
        outcomes,
    })
}

/// How many gates passed after `lines`: all of them once the closing stage is reached, and fewer
/// when an `anytime` outcome ended the run first.
#[must_use]
pub fn passed_gates(gates: &Gates, lines: &[GateLine]) -> usize {
    walk(gates, lines).at
}
