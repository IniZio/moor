//! Deciding the outcome: moor runs the checks of the outcomes due, in order, and the first whose
//! checks all pass is the run's. The agent never claims one.
//!
//! At the closing stage a result is always written: the outcome that held, or that none did,
//! which fails the try and is steered to the agent. While a gate waits for a quiet agent only the
//! `anytime` outcomes are tried, and when none holds the pass reminds the agent as it would
//! without them.

use moor_core::gate::{DueOutcomes, GateLine, OutcomeName, Tried};
use moor_core::workflow::StepId;
use moor_core::BoxId;

use super::checks::{taken, tell};
use super::remind::remind;
use crate::cx::Cx;
use crate::ops::common::check::{execute, Ended, CHECK_LIMIT};

/// The outcomes a job tries, with their commands, at a stage and try.
pub(super) struct Due {
    /// The gate in hand, or the closing stage.
    stage: StepId,
    /// The try.
    try_n: u8,
    /// Whether this is the closing stage, where a result is always written.
    closing: bool,
    /// Each outcome with its check commands, in order.
    outcomes: Vec<(OutcomeName, Vec<String>)>,
}

impl Due {
    /// What `due` asks to run.
    pub(super) fn of(due: &DueOutcomes<'_>) -> Self {
        Self {
            stage: due.stage.clone(),
            try_n: due.try_n,
            closing: due.stage.as_str() == moor_core::gate::CLOSING,
            outcomes: due
                .outcomes
                .iter()
                .map(|o| {
                    (
                        o.name.clone(),
                        o.checks.iter().map(|c| c.run.clone()).collect(),
                    )
                })
                .collect(),
        }
    }
}

/// How trying the outcomes went.
enum Found {
    /// This one held.
    Held(OutcomeName),
    /// None held; each with the check that stopped it.
    None(Vec<Tried>),
}

/// Tries the outcomes of `due` in Box `id` and writes what it found as line `n`. Returns what to
/// log.
pub(super) fn run(cx: &Cx, (id, n): (BoxId, i64), due: &Due) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let Some(found) = find(cx, id, due, &mut out) else {
        return out;
    };
    let line = match found {
        Found::Held(outcome) => GateLine::Held {
            gate: due.stage.clone(),
            try_n: due.try_n,
            outcome,
        },
        Found::None(tried) if due.closing => GateLine::NoneHeld {
            gate: due.stage.clone(),
            try_n: due.try_n,
            tried,
        },
        Found::None(_) => {
            if let Err(e) = remind_now(cx, id) {
                out.push(format!("outcomes: could not remind the agent: {}", e.error));
            }
            return out;
        }
    };
    match store.append_gate_line(id, n, cx.clock().now(), &line) {
        Ok(()) => {
            if let Err(e) = tell(cx, store, id) {
                out.push(format!("outcomes: could not tell the agent: {}", e.error));
            }
        }
        Err(e) if taken(&e) => {}
        Err(e) => out.push(format!("outcomes: {e}")),
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

/// Runs the outcomes' checks in order until one outcome holds. A lost stream stops the search, so
/// a later outcome never wins for want of an answer. None when the Box is gone or a check did not
/// run, which decides nothing: the next pass tries again.
fn find(cx: &Cx, id: BoxId, due: &Due, out: &mut Vec<String>) -> Option<Found> {
    let timeout = u32::try_from(CHECK_LIMIT.as_secs()).unwrap_or(u32::MAX);
    let mut tried = Vec::new();
    'outcomes: for (name, commands) in &due.outcomes {
        for (at, command) in commands.iter().enumerate() {
            let ran = execute(cx, id, command, timeout)?;
            if let Some(why) = &ran.error {
                out.push(format!(
                    "outcome {name}: no result from Box {}: {why}",
                    id.get()
                ));
            }
            let (exit, tail) = match ran.ended {
                Ended::NotRun { .. } => return None,
                Ended::Lost => (None, String::new()),
                Ended::Exit { exit: 0, .. } => continue,
                Ended::Exit { exit, tail } => (Some(exit), tail),
            };
            let check = u8::try_from(at).unwrap_or(u8::MAX);
            tried.push(Tried {
                outcome: name.clone(),
                check,
                exit,
                tail,
            });
            if exit.is_none() {
                break 'outcomes;
            }
            continue 'outcomes;
        }
        return Some(Found::Held(name.clone()));
    }
    Some(Found::None(tried))
}

/// Reminds the agent of Box `id` as the pass would have, from the lines as they are now.
fn remind_now(cx: &Cx, id: BoxId) -> Result<(), crate::error::CommandError> {
    let store = cx.store()?;
    let (Some(row), Some(gates)) = (store.box_row(id)?, store.box_gates(id)?) else {
        return Ok(());
    };
    remind(cx, store, &row, (&gates, &store.gate_lines(id)?))
}
