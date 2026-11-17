//! Where a run goes next: from a quiet or ended agent, or from a check's result.

use super::finished::{RunBoxFacts, QUIET_BEFORE_FINISHED};
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;
use crate::workflow::{Answer, HistoryEntry, Plan, StepId, StepKind, StepReport, StepResult};

/// How many lost exec streams one try of a check may have. The next one follows `fail`.
pub const CHECK_LOSSES: usize = 2;

/// What the tick saw of the run's agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The agent is idle and has been long enough to count as done with its step.
    Idle,
    /// The agent's session ended, by a crash or an exit, with no report.
    Ended,
}

/// What a run does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// Nothing yet.
    Wait,
    /// Send the current step again and record one reminder.
    Remind,
    /// Run the current step's check now.
    RunCheck,
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
    /// The run is at an approve step: it waits for the owner's answer.
    Ask,
    /// The owner said no at step `at`, which has no `fail` edge.
    Declined {
        /// The approve step.
        at: StepId,
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
    /// The agent stopped twice at step `at` without finishing it.
    Incomplete {
        /// The step.
        at: StepId,
    },
}

/// Which edge of a step a run leaves by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// The `ok` edge.
    Ok,
    /// The `fail` edge.
    Fail,
}

/// What a run at the step it last entered does next, given its `history` and what the tick saw
/// of its agent in `event`.
///
/// The pointer is the last `Entered` line, and what happened in this try is every line after it,
/// so the move comes from history alone and a restarted controller decides as the old one did.
///
/// - A work step waits for the agent's report, which [`report_move`] decides. An agent that is
///   quiet, or whose session ended, without one is reminded once; quiet or ended again in the
///   same try ends the run Incomplete. `event` makes no difference: the two readings of "no
///   report" are treated alike.
/// - A deliver step is the same, except in the plan of a workflow nobody planned
///   ([`Plan::is_single`]), where it delivers on either.
/// - A check step runs. Its recorded exit decides: 0 follows `ok`, any other follows `fail`. A
///   lost stream is no result, so the check runs again, and the second lost stream in one try
///   follows `fail`.
/// - An approve step asks: the run waits for its owner, who answers with [`answer_move`].
#[must_use]
pub fn advance(plan: &Plan, history: &[HistoryEntry], _event: Event) -> Move {
    let Some((pos, at)) = history.iter().enumerate().rev().find_map(|(i, e)| match e {
        HistoryEntry::Entered { step, .. } => Some((i, step)),
        _ => None,
    }) else {
        return Move::Wait;
    };
    let Some(step) = plan.step(at) else {
        return Move::Wait;
    };
    let this_try = history.get(pos + 1..).unwrap_or_default();
    let entered = |s: &StepId| entered_count(history, s);
    let reminded = this_try
        .iter()
        .any(|e| matches!(e, HistoryEntry::Reminded { .. }));
    // A step the agent reports waits for the report. A quiet agent is reminded once, and quiet
    // again after the reminder ends the run. A workflow nobody planned has no report to wait
    // for: its one deliver step delivers when the agent is quiet.
    let unreported = |reminded: bool| {
        if reminded {
            Move::Incomplete { at: at.clone() }
        } else {
            Move::Remind
        }
    };
    match &step.kind {
        StepKind::Deliver { .. } if plan.is_single() => Move::Delivered,
        StepKind::Work { .. } | StepKind::Deliver { .. } => unreported(reminded),
        StepKind::Check { .. } => {
            let lost = this_try
                .iter()
                .filter(|e| matches!(e, HistoryEntry::CheckLost { .. }))
                .count();
            let exit = this_try.iter().rev().find_map(|e| match e {
                HistoryEntry::Checked { exit, .. } => Some(*exit),
                _ => None,
            });
            match exit {
                Some(0) => route(plan, at, Edge::Ok, &entered, String::new()),
                Some(code) => route(
                    plan,
                    at,
                    Edge::Fail,
                    &entered,
                    format!("check {at} exited {code} and has no fail edge"),
                ),
                None if lost >= CHECK_LOSSES => route(
                    plan,
                    at,
                    Edge::Fail,
                    &entered,
                    format!("check {at} lost its exec stream {lost} times and has no fail edge"),
                ),
                None => Move::RunCheck,
            }
        }
        StepKind::Approve { .. } => Move::Ask,
    }
}

/// The move after the agent's passed `report` at the step the run last entered: along the `ok`
/// edge for ok, along the `fail` edge for fail, and the end of the run for ok at a deliver step.
///
/// A step with no `fail` edge, or a fail target out of tries, fails the run with the report's
/// note as the reason. A report of a step the run is not at is none of this function's business.
#[must_use]
pub fn report_move(plan: &Plan, history: &[HistoryEntry], report: &StepReport) -> Move {
    let Some(at) = history.iter().rev().find_map(|e| match e {
        HistoryEntry::Entered { step, .. } => Some(step),
        _ => None,
    }) else {
        return Move::Wait;
    };
    let entered = |s: &StepId| entered_count(history, s);
    let reason = format!("step {at} failed: {}", report.note.trim());
    match report.result {
        StepResult::Ok => route(plan, at, Edge::Ok, &entered, String::new()),
        StepResult::Fail => failed(plan, at, &entered, reason),
    }
}

/// The move after the owner's `answer` at the approve step the run last entered: yes follows the
/// `ok` edge, no follows the `fail` edge, and a no at a step with none ends the run declined.
#[must_use]
pub fn answer_move(plan: &Plan, history: &[HistoryEntry], answer: Answer) -> Move {
    let Some(at) = history.iter().rev().find_map(|e| match e {
        HistoryEntry::Entered { step, .. } => Some(step),
        _ => None,
    }) else {
        return Move::Wait;
    };
    let entered = |s: &StepId| entered_count(history, s);
    let no_fail_edge = plan
        .step(at)
        .and_then(|s| s.kind.next())
        .is_some_and(|n| n.fail.is_none());
    match answer {
        Answer::Yes => route(plan, at, Edge::Ok, &entered, String::new()),
        Answer::No if no_fail_edge => Move::Declined { at: at.clone() },
        Answer::No => route(plan, at, Edge::Fail, &entered, String::new()),
    }
}

/// The move after the code gate refused a report of the step the run last entered: nothing for
/// the first refusal of a try, and the step failing for the second.
#[must_use]
pub fn refusal_move(plan: &Plan, history: &[HistoryEntry]) -> Move {
    let Some((pos, at)) = history.iter().enumerate().rev().find_map(|(i, e)| match e {
        HistoryEntry::Entered { step, .. } => Some((i, step)),
        _ => None,
    }) else {
        return Move::Wait;
    };
    let refused = history
        .get(pos + 1..)
        .unwrap_or_default()
        .iter()
        .any(|e| matches!(e, HistoryEntry::GateRefused { .. }));
    if !refused {
        return Move::Wait;
    }
    let entered = |s: &StepId| entered_count(history, s);
    failed(
        plan,
        at,
        &entered,
        format!("step {at}: the agent's report was refused twice"),
    )
}

/// How often the run in `history` has entered `step`.
fn entered_count(history: &[HistoryEntry], step: &StepId) -> u8 {
    let n = history
        .iter()
        .filter(|e| matches!(e, HistoryEntry::Entered { step: s, .. } if s == step))
        .count();
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// The move when step `at` failed with `reason`: its `fail` edge, or the end of the run as
/// failed when it has none. A deliver step has none.
fn failed(plan: &Plan, at: &StepId, entered: &dyn Fn(&StepId) -> u8, reason: String) -> Move {
    if plan.step(at).is_some_and(|s| s.kind.next().is_none()) {
        return Move::Fail {
            at: at.clone(),
            reason,
            exhausted: Vec::new(),
        };
    }
    route(plan, at, Edge::Fail, entered, reason)
}

/// Where a run at `at` goes when its agent is quiet: along its `ok` edge.
///
/// A step entered `max_tries` times already is exhausted, and the run follows its `fail` edge
/// instead, once: a fail target out of tries ends the run, so every run ends within the sum of
/// the tries.
///
/// `entered` says how many times the run has entered a step.
#[must_use]
pub fn quiet_move(plan: &Plan, at: &StepId, entered: &dyn Fn(&StepId) -> u8) -> Move {
    route(plan, at, Edge::Ok, entered, String::new())
}

/// The move from `at` along `edge`. `no_edge` is the failure's reason when `at` has no such edge.
fn route(
    plan: &Plan,
    at: &StepId,
    edge: Edge,
    entered: &dyn Fn(&StepId) -> u8,
    no_edge: String,
) -> Move {
    let next = plan.step(at).and_then(|s| s.kind.next());
    let target = match (edge, next) {
        (_, None) => return Move::Delivered,
        (Edge::Ok, Some(n)) => Some(n.ok.clone()),
        (Edge::Fail, Some(n)) => n.fail.clone(),
    };
    let Some(mut target) = target else {
        return Move::Fail {
            at: at.clone(),
            reason: no_edge,
            exhausted: Vec::new(),
        };
    };
    let mut exhausted = Vec::new();
    for hop in 0..2 {
        let Some(step) = plan.step(&target) else {
            break;
        };
        let tried = entered(&target);
        if tried < step.max_tries.get() {
            return Move::Enter {
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
    Move::Fail {
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
