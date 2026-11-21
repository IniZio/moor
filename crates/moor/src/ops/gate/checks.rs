//! The gate pass: runs the checks of a Box's gates once its agent is quiet, one after another.
//!
//! The lines are the state. A pass reads them, and the check that is due goes to a thread of its
//! own, so a long check never holds up the pass. The line it writes takes the number the pass read
//! the lines at, and the store's key on that number is the compare-and-set: of two checks that were
//! started from the same lines only the first writes, and the other drops its result. A controller
//! that stops between a check and its line runs the check again.
//!
//! A failed check ends the try, as `status()` reads it. When tries remain, the end of its output is
//! steered to the agent, which then fixes the work and goes quiet again.

use moor_core::gate::{due_check, status, GateLine, GateStatus, Gates};
use moor_core::request::BoxTarget;
use moor_core::workflow::StepId;
use moor_core::{BoxId, SteerReq};
use moor_schema::tables::BoxGateLines;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::check::{execute, Claim, Ended, Flight, CHECK_LIMIT};
use crate::ops::common::quiet::{box_facts, quiet_for_check};
use crate::ops::steer;
use crate::store::{BoxRow, Store, StoreError};

/// A check to run: the gate, the try and the Box, and the number of the line it writes.
pub(crate) struct Job {
    /// The Box to run it in.
    box_id: BoxId,
    /// The gate.
    gate: StepId,
    /// The try.
    try_n: u8,
    /// The check's place among the gate's checks.
    check: u8,
    /// The command.
    command: String,
    /// The number of the line the result takes: the one after those the pass read.
    n: i64,
    /// Held until the job is dropped, so no pass starts a check of the Box twice.
    _flight: Flight,
}

/// Looks at each open Box with gates, handing the check that is due to `checks`, which must not
/// wait for it. A failure of the try just ended goes to the agent first. Returns what to log.
///
/// # Errors
/// A store failure listing the Boxes.
pub(crate) fn pass(
    cx: &Cx,
    store: &Store,
    draining: &dyn Fn() -> bool,
    checks: &dyn Fn(Job),
) -> Result<Vec<String>, CommandError> {
    let gated = store.gated_box_ids()?;
    let mut out = Vec::new();
    for row in store.open_boxes()? {
        if draining() {
            break;
        }
        if !gated.contains(&row.id) {
            continue;
        }
        if let Err(e) = settle(cx, store, &row, checks) {
            out.push(format!("gates of {}: {}", row.name.as_str(), e.error));
        }
    }
    Ok(out)
}

/// One Box's pass.
fn settle(cx: &Cx, store: &Store, row: &BoxRow, checks: &dyn Fn(Job)) -> Result<(), CommandError> {
    // The claim comes before the lines are read: a check thread writes its line and only then
    // drops its flight, so a pass that holds the claim reads every line a finished check wrote.
    let Some(flight) = Flight::claim(cx, Claim::Box(row.id)) else {
        return Ok(());
    };
    let Some(gates) = store.box_gates(row.id)? else {
        return Ok(());
    };
    let lines = store.gate_lines(row.id)?;
    if tell_failure(cx, store, row, (&gates, &lines))? {
        return Ok(());
    }
    let Some(due) = due_check(&gates, &lines) else {
        return Ok(());
    };
    let steered = store.last_event_at(row.id, &["steered"])?;
    let facts = box_facts(store, row)?;
    if !quiet_for_check(&facts, steered, cx.clock().now()) || !store.unsent(row.id)?.is_empty() {
        return Ok(());
    }
    let Some(command) = due.gate.checks.get(usize::from(due.check)) else {
        return Ok(());
    };
    checks(Job {
        box_id: row.id,
        gate: due.gate.name.clone(),
        try_n: due.try_n,
        check: due.check,
        command: command.clone(),
        n: i64::try_from(lines.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1),
        _flight: flight,
    });
    Ok(())
}

/// What the agent is told about the check whose failure ended the newest try, when a failure did
/// and tries remain: the check, why it failed, and the end of its output.
fn failure_text(gates: &Gates, lines: &[GateLine]) -> Option<String> {
    let GateStatus::Open { gate, try_n, .. } = status(gates, lines) else {
        return None;
    };
    let (failed_try, check, why, tail) = match lines.last()? {
        GateLine::Checked {
            gate: g,
            try_n,
            check,
            exit,
            tail,
        } if g == &gate && *exit != 0 => (*try_n, *check, format!("exited {exit}"), tail.as_str()),
        GateLine::CheckLost {
            gate: g,
            try_n,
            check,
        } if g == &gate => (*try_n, *check, "lost its output twice".to_owned(), ""),
        _ => return None,
    };
    if failed_try + 1 != try_n {
        return None;
    }
    let command = gates
        .iter()
        .find(|g| g.name == gate)?
        .checks
        .get(usize::from(check))?;
    let mut text = format!(
        "Gate `{gate}` is not passed: check {} (`{command}`) {why}.",
        check + 1
    );
    if !tail.trim().is_empty() {
        text.push_str("\n\nThe end of its output:\n");
        text.push_str(tail.trim_end());
    }
    text.push_str("\n\nFix it, then stop. moor runs the checks again once you are quiet.");
    Some(text)
}

/// Steers the agent of `row` the failure that ended the newest try, unless it was told already: a
/// steer came after the line, or one waits in the inbox. True when it sent one.
fn tell_failure(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<bool, CommandError> {
    let Some(text) = failure_text(gates, lines) else {
        return Ok(false);
    };
    let wrote = store.newest_gate_line_at(row.id)?;
    let steered = store.last_event_at(row.id, &["steered"])?;
    let told = steered
        .zip(wrote)
        .is_some_and(|(steered, wrote)| steered >= wrote);
    if told || !store.unsent(row.id)?.is_empty() {
        return Ok(false);
    }
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(text),
        menu: None,
        interrupt: false,
        ask: None,
    };
    cx.for_box(row.user, || steer::run(cx, &req))?;
    Ok(true)
}

/// Whether `e` says another line took the number.
fn taken(e: &StoreError) -> bool {
    matches!(e, StoreError::Db(db) if db.unique_key().is_some_and(|k| {
        k.is(&BoxGateLines::Table, &[BoxGateLines::Box, BoxGateLines::N])
    }))
}

/// Runs the check of `job` in its Box as the Box's owner and writes how it ended, then tells the
/// agent when that failed the try. A result whose line number was taken meanwhile is dropped.
/// Returns what to log.
pub(crate) fn run(cx: &Cx, job: &Job) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let timeout = u32::try_from(CHECK_LIMIT.as_secs()).unwrap_or(u32::MAX);
    let Some(ran) = execute(cx, job.box_id, &job.command, timeout) else {
        return out;
    };
    let name = job.gate.as_str();
    if let Some(why) = &ran.error {
        out.push(format!(
            "gate {name}: no result from Box {}: {why}",
            job.box_id.get()
        ));
    }
    let (gate, try_n, check) = (job.gate.clone(), job.try_n, job.check);
    let line = match ran.ended {
        Ended::Lost => GateLine::CheckLost { gate, try_n, check },
        Ended::Exit { exit, tail } => GateLine::Checked {
            gate,
            try_n,
            check,
            exit,
            tail,
        },
    };
    match store.append_gate_line(job.box_id, job.n, cx.clock().now(), &line) {
        Ok(()) => {
            if let Err(e) = tell(cx, store, job.box_id) {
                out.push(format!(
                    "gate {name}: could not tell the agent: {}",
                    e.error
                ));
            }
        }
        Err(e) if taken(&e) => {}
        Err(e) => out.push(format!("gate {name}: {e}")),
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

/// Tells the agent of Box `id` what failed, when its newest line ended a try.
fn tell(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let (Some(row), Some(gates)) = (store.box_row(id)?, store.box_gates(id)?) else {
        return Ok(());
    };
    tell_failure(cx, store, &row, (&gates, &store.gate_lines(id)?)).map(drop)
}
