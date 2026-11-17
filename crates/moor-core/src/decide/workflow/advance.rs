//! Where a run goes when its agent goes quiet, and whether a quiet reading counts.

use super::finished::{RunBoxFacts, QUIET_BEFORE_FINISHED};
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;
use crate::workflow::{Plan, StepId};

/// What a quiet agent at step `at` means for its run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuietMove {
    /// The step is a deliver step, so the run is done.
    Delivered,
    /// The run goes on to `step`, for the `try_n`th time. Each step in `exhausted` was due but
    /// had used up its tries, so the run was sent on along its fail edge.
    Enter {
        /// The step to enter.
        step: StepId,
        /// How often the run has entered it, this entry included.
        try_n: u8,
        /// Steps passed over for lack of tries, in order.
        exhausted: Vec<StepId>,
    },
    /// The run goes nowhere: a step is out of tries and has no fail edge, or its fail target is
    /// out too.
    Fail {
        /// The step that stopped it.
        at: StepId,
        /// Why.
        reason: String,
        /// Steps passed over for lack of tries, in order.
        exhausted: Vec<StepId>,
    },
}

/// Where a run at `at` goes when its agent is quiet.
///
/// A step passes along its `ok` edge. A step
/// entered `max_tries` times already is exhausted, and the run follows its `fail` edge instead,
/// once: a fail target out of tries ends the run, so every run ends within the sum of the tries.
///
/// `entered` says how many times the run has entered a step.
#[must_use]
pub fn quiet_move(plan: &Plan, at: &StepId, entered: &dyn Fn(&StepId) -> u8) -> QuietMove {
    let Some(next) = plan.step(at).and_then(|s| s.kind.next()) else {
        return QuietMove::Delivered;
    };
    let mut exhausted = Vec::new();
    let mut target = next.ok.clone();
    for hop in 0..2 {
        let Some(step) = plan.step(&target) else {
            break;
        };
        let tried = entered(&target);
        if tried < step.max_tries.get() {
            return QuietMove::Enter {
                step: target,
                try_n: tried + 1,
                exhausted,
            };
        }
        exhausted.push(target.clone());
        let fail = step.kind.next().and_then(|n| n.fail.clone());
        match (hop, fail) {
            (0, Some(f)) => target = f,
            _ => break,
        }
    }
    let last = exhausted.last().cloned().unwrap_or_else(|| at.clone());
    QuietMove::Fail {
        reason: format!("step {last} ran out of tries"),
        at: last,
        exhausted,
    }
}

/// Whether a quiet reading of the Box, `facts`, counts for the step sent at `sent`.
///
/// The facts of a steer still on its way are the facts of the step before, so a
/// reading counts only once the agent has been idle since the send, or has been ended for as
/// long as a quiet agent takes to count as finished.
#[must_use]
pub fn quiet_since(facts: &RunBoxFacts, sent: Ts, now: Ts) -> bool {
    match facts.session {
        SessionState::Idle => facts.idle_since.is_some_and(|since| since >= sent),
        SessionState::Ended => {
            let waited = now.as_millis().saturating_sub(sent.as_millis());
            u128::try_from(waited).is_ok_and(|w| w >= QUIET_BEFORE_FINISHED.as_millis())
        }
        SessionState::Starting | SessionState::Working => false,
    }
}

#[cfg(test)]
mod tests;
