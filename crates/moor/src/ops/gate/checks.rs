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

use moor_core::domain::event::Origin;
use moor_core::gate::{
    due_check, due_outcomes, status, CheckOf, GateLine, GateStatus, Gates, Unanswered,
};
use moor_core::request::BoxTarget;
use moor_core::view::steer::SteerValue;
use moor_core::workflow::{Answer, StepId};
use moor_core::{BoxId, SteerReq};
use moor_schema::tables::BoxGateLines;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::check::{execute, Claim, Ended, Flight, CHECK_LIMIT};
use crate::ops::common::quiet::settled;
use crate::ops::gate::ask::ask;
use crate::ops::gate::decide::{self, Due};
use crate::ops::gate::handoff::hand_over;
use crate::ops::gate::remind::remind;
use crate::ops::steer;
use crate::store::{Appended, BoxRow, RunOfBox, Store, StoreError};

/// What to run in a Box, and the number of the line the result takes.
pub(crate) struct Job {
    /// The Box to run it in.
    box_id: BoxId,
    /// What to run.
    work: Work,
    /// The number of the line the result takes: the one after those the pass read.
    n: i64,
    /// Held until the job is dropped, so no pass starts a check of the Box twice.
    _flight: Flight,
}

/// What a job runs.
enum Work {
    /// One check of a gate.
    Check(GateCheck),
    /// The checks of the outcomes due, to find the one that holds.
    Outcomes(Due),
}

/// One check of a gate, due at a try.
pub(super) struct GateCheck {
    /// The gate.
    gate: StepId,
    /// The try.
    try_n: u8,
    /// The check's place among the gate's checks.
    check: u8,
    /// The command.
    command: String,
}

/// Looks at each open Box with gates, handing the check that is due to `checks`, which must not
/// wait for it. A Box whose workflow run ended is left alone: a cancelled, failed or timed out run
/// has nobody to remind or check for. A failure of the try just ended goes to the agent first. Returns what to log.
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
        match store.run_of_box(row.id) {
            Ok(RunOfBox::Ended(_)) => continue,
            Err(e) => {
                out.push(format!("gates of {}: {e}", row.name.as_str()));
                continue;
            }
            Ok(RunOfBox::NoRun | RunOfBox::Open) => {}
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
    if tell_failure(cx, store, row, (&gates, &lines))?.is_some() {
        return Ok(());
    }
    let n = i64::try_from(lines.len())
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    let Some(work) = due_work(cx, store, row, (&gates, &lines))? else {
        return Ok(());
    };
    if !settled(cx, store, row)? {
        return Ok(());
    }
    checks(Job {
        box_id: row.id,
        work,
        n,
        _flight: flight,
    });
    Ok(())
}

/// What the pass runs next in Box `row`: the check that is due, after the `anytime` outcomes when
/// there are any, else the outcomes that are due once the owner is asked and the agent told of a
/// hand-over. None when nothing is to run, after the agent is reminded if it waits for one.
fn due_work(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<Option<Work>, CommandError> {
    if let Some(due) = due_check(gates, lines) {
        let Some(command) = due.gate.checks.get(usize::from(due.check)) else {
            return Ok(None);
        };
        let check = GateCheck {
            gate: due.gate.name.clone(),
            try_n: due.try_n,
            check: due.check,
            command: command.run.clone(),
        };
        return Ok(Some(match due_outcomes(gates, lines) {
            Some(anytime) => Work::Outcomes(Due::of(&anytime).then(check)),
            None => Work::Check(check),
        }));
    }
    ask(cx, store, row, (gates, lines))?;
    if hand_over(cx, store, row, (gates, lines))? {
        return Ok(None);
    }
    let Some(due) = due_outcomes(gates, lines) else {
        remind(cx, store, row, (gates, lines))?;
        return Ok(None);
    };
    Ok(Some(Work::Outcomes(Due::of(&due))))
}

/// Whether `line` is one the agent's own doings wrote in try `try_n`, which a failure that ended
/// the try before it may be followed by.
const fn of_this_try(line: &GateLine, try_n: u8) -> bool {
    matches!(
        line,
        GateLine::Submitted { try_n: t, .. }
            | GateLine::Refused { try_n: t, .. }
            | GateLine::Reminded { try_n: t, .. }
            | GateLine::Quiet { try_n: t, .. } if *t == try_n
    )
}

/// The place of the line that failed a check, or the owner's no, which ended the try before the one
/// in hand, when tries remain, and what the agent is told of it. The try in hand may have lines of its own after the
/// failure, such as a refused report.
fn failure_text(gates: &Gates, lines: &[GateLine]) -> Option<(usize, String)> {
    let GateStatus::Open { gate, try_n, .. } = status(gates, lines) else {
        return None;
    };
    let (at, line) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| !of_this_try(l, try_n))?;
    let waiting = || gates.iter().find(|g| g.name == gate);
    let (failed_try, text) = match line {
        GateLine::NoneHeld {
            gate: g,
            try_n,
            tried,
        } if g == &gate => (*try_n, gates.none_held_steer(tried)),
        GateLine::Checked {
            gate: g,
            try_n,
            check,
            exit,
            tail,
        } if g == &gate && *exit != 0 => (
            *try_n,
            waiting()?.failed_check_steer(*check, (&format!("exited {exit}"), tail)),
        ),
        GateLine::Answered {
            gate: g,
            try_n,
            answer: Answer::No,
            note,
            ..
        } if g == &gate => (*try_n, waiting()?.owner_no_steer(note)),
        _ => return None,
    };
    if failed_try + 1 != try_n {
        return None;
    }
    Some((at, text))
}

/// Steers the agent of `row` the failure that ended the try before the one in hand, unless it was
/// told already: a steer came after the failure's line, or one waits in the inbox. What the steer
/// came to when it sent one.
fn tell_failure(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<Option<SteerValue>, CommandError> {
    let Some((at, text)) = failure_text(gates, lines) else {
        return Ok(None);
    };
    let wrote = store.gate_line_at(
        row.id,
        i64::try_from(at).unwrap_or(i64::MAX).saturating_add(1),
    )?;
    let steered = store.last_event_at(row.id, &["steered"])?;
    let told = steered
        .zip(wrote)
        .is_some_and(|(steered, wrote)| steered >= wrote);
    if told || !store.unsent(row.id)?.is_empty() {
        return Ok(None);
    }
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(text),
        menu: None,
        interrupt: false,
        ask: None,
    };
    cx.for_box(row.user, || steer::run_as(cx, &req, Origin::Moor))
        .map(Some)
}

/// What to log when a job's result is dropped because the run of Box `id` ended while it ran.
pub(super) fn dropped(what: &str, id: BoxId) -> String {
    format!(
        "{what}: dropped the result for Box {}: its run has ended",
        id.get()
    )
}

/// Whether `e` says another line took the number.
pub(super) fn taken(e: &StoreError) -> bool {
    matches!(e, StoreError::Db(db) if db.unique_key().is_some_and(|k| {
        k.is(&BoxGateLines::Table, &[BoxGateLines::Box, BoxGateLines::N])
    }))
}

/// Runs `job` in its Box as the Box's owner and writes how it ended. Returns what to log.
pub(crate) fn run(cx: &Cx, job: &Job) -> Vec<String> {
    match &job.work {
        Work::Check(check) => run_check(cx, (job.box_id, job.n), check),
        Work::Outcomes(due) => decide::run(cx, (job.box_id, job.n), due),
    }
}

/// Runs `check` in Box `id` and writes how it ended as line `n`, then tells the agent when that
/// failed the try. A result whose line number was taken meanwhile is dropped.
pub(super) fn run_check(cx: &Cx, (id, n): (BoxId, i64), check: &GateCheck) -> Vec<String> {
    let GateCheck {
        gate,
        try_n,
        check,
        command,
    } = check;
    let (try_n, check) = (*try_n, *check);
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let timeout = u32::try_from(CHECK_LIMIT.as_secs()).unwrap_or(u32::MAX);
    let Some(ran) = execute(cx, id, command, timeout) else {
        return out;
    };
    let name = gate.as_str();
    if let Some(why) = &ran.error {
        out.push(format!(
            "gate {name}: no result from Box {}: {why}",
            id.get()
        ));
    }
    let gate = gate.clone();
    let line = match unanswered(ran.ended, ran.error) {
        Ok((exit, tail)) => GateLine::Checked {
            gate,
            try_n,
            check,
            exit,
            tail,
        },
        Err(why) => GateLine::NoAnswer {
            gate,
            try_n,
            check: CheckOf::Gate { check },
            why,
        },
    };
    match store.append_job_line(id, n, cx.clock().now(), &line) {
        Ok(Appended::Written) => {
            if let Err(e) = tell(cx, store, id) {
                out.push(format!(
                    "gate {name}: could not tell the agent: {}",
                    e.error
                ));
            }
        }
        Ok(Appended::RunEnded) => out.push(dropped(&format!("gate {name}"), id)),
        Err(e) if taken(&e) => {}
        Err(e) => out.push(format!("gate {name}: {e}")),
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

/// The exit code and tail of a check that ended as `ended`, or why moor got no answer from it,
/// with the exec's `error` as the reason it did not run.
pub(super) fn unanswered(ended: Ended, error: Option<String>) -> Result<(i32, String), Unanswered> {
    match ended {
        Ended::Exit { exit, tail } => Ok((exit, tail)),
        Ended::Lost => Err(Unanswered::Lost),
        Ended::NotRun { tail } => Err(Unanswered::NotRun {
            reason: error.unwrap_or(tail),
        }),
    }
}

/// Tells the agent of Box `id` what failed, when its newest line ended a try.
pub(super) fn tell(cx: &Cx, store: &Store, id: BoxId) -> Result<Option<SteerValue>, CommandError> {
    let (Some(row), Some(gates)) = (store.box_row(id)?, store.box_gates(id)?) else {
        return Ok(None);
    };
    tell_failure(cx, store, &row, (&gates, &store.gate_lines(id)?))
}
