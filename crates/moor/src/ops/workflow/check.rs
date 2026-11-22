//! Running a check step: the command goes through the exec op in the Box's work place, and its
//! result is a history line the next tick reads.
//!
//! The act comes first and its line after, so a controller that stops in between runs the check
//! again. A cut link is no result: it is recorded as `CheckLost`, and the decider reruns it. Any
//! other error is a check that did not pass, with its reason in the `Checked` line.

use moor_core::workflow::{HistoryEntry, RunId, StepId};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::ops::common::check::{execute, Ended, Flight};

/// A check to run: the step, the Box and the deadline.
pub(crate) struct CheckJob {
    /// The run.
    pub(super) run: RunId,
    /// The Box to run it in.
    pub(super) box_id: BoxId,
    /// The step and its try.
    pub(super) at: (StepId, u8),
    /// The shell command.
    pub(super) command: String,
    /// The deadline: the smaller of the run's remaining timeout and [`CHECK_LIMIT`](crate::ops::common::check::CHECK_LIMIT).
    pub(super) timeout_secs: u32,
    /// Held until the job is dropped, so no tick starts the same check twice.
    pub(super) _flight: Flight,
}

/// The history line for a check that ended as `ended` says.
fn outcome_line(job: &CheckJob, ended: Ended) -> HistoryEntry {
    let (step, try_n) = (job.at.0.clone(), job.at.1);
    match ended {
        Ended::Lost => HistoryEntry::CheckLost { step, try_n },
        Ended::Exit { exit, tail } => HistoryEntry::Checked {
            step,
            try_n,
            exit,
            tail,
            check: None,
        },
    }
}

/// Runs the check of `job` in its Box as the Box's owner and records how it ended. Returns what to
/// log.
pub(crate) fn run_check(cx: &Cx, job: &CheckJob) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let Some(ran) = execute(cx, job.box_id, &job.command, job.timeout_secs) else {
        return out;
    };
    if let Some(why) = &ran.error {
        out.push(format!(
            "workflow check {}: no result from Box {}: {why}",
            job.at.0,
            job.box_id.get(),
        ));
    }
    let line = outcome_line(job, ran.ended);
    let (from, key) = (&job.at, job.run);
    let at = ((&from.0, from.1), (&from.0, from.1));
    if let Err(e) = store.step_workflow_run(key, at, (&[line], cx.clock().now())) {
        out.push(format!("workflow check {}: {e}", job.at.0));
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}
