//! Running a check step: the command goes through the exec op in the Box's work place, and its
//! result is a history line the next tick reads.
//!
//! The act comes first and its line after, so a controller that stops in between runs the check
//! again. A cut link is no result: it is recorded as `CheckLost`, and the decider reruns it. Any
//! other error is a check that did not pass, with its reason in the `Checked` line.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::name::ForgeRepo;
use moor_core::request::BoxTarget;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::workflow::{HistoryEntry, RunId, StepId, WorkflowId};
use moor_core::{BoxId, ExecReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::exec;

/// The longest a check may run, however long the run has left.
pub(super) const CHECK_LIMIT: Duration = Duration::from_mins(30);

/// How much of a check's output the history keeps: its end.
pub(crate) const TAIL_BYTES: usize = 4096;

/// What the history records for a check stopped at its deadline, which has no exit code.
const DEADLINE_EXIT: i32 = -1;

/// What the history records for a check that never got a result and was not cut off by the link.
const NOT_RUN_EXIT: i32 = -2;

/// A check to run: the step, the Box and the deadline.
pub(crate) struct CheckJob {
    /// The workflow.
    pub(super) workflow: WorkflowId,
    /// The repo.
    pub(super) repo: ForgeRepo,
    /// The due time of the run.
    pub(super) due_at: Ts,
    /// The Box to run it in.
    pub(super) box_id: BoxId,
    /// The step and its try.
    pub(super) at: (StepId, u8),
    /// The shell command.
    pub(super) command: String,
    /// The deadline: the smaller of the run's remaining timeout and [`CHECK_LIMIT`].
    pub(super) timeout_secs: u32,
    /// Held until the job is dropped, so no tick starts the same check twice.
    pub(super) _flight: Flight,
}

/// The checks running in this process, by state directory and run, so two ticks never run one
/// check at once. A controller restart forgets them, and the check then runs again.
fn flights() -> &'static Mutex<HashSet<(PathBuf, RunId)>> {
    static FLIGHTS: OnceLock<Mutex<HashSet<(PathBuf, RunId)>>> = OnceLock::new();
    FLIGHTS.get_or_init(Mutex::default)
}

/// A run whose check is running; dropping it ends the claim.
pub(crate) struct Flight(PathBuf, RunId);

impl Flight {
    /// Claims `run`; none when its check is already running.
    pub(super) fn claim(cx: &Cx, run: RunId) -> Option<Self> {
        let dir = cx.state_dir().map(PathBuf::from).unwrap_or_default();
        let fresh = flights()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((dir.clone(), run));
        fresh.then_some(Self(dir, run))
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        flights()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(self.0.clone(), self.1));
    }
}

/// The end of a command's output, shared by its two streams.
#[derive(Clone, Default)]
struct Tail(Arc<Mutex<Vec<u8>>>);

impl Write for Tail {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        bytes.extend_from_slice(buf);
        let extra = bytes.len().saturating_sub(TAIL_BYTES);
        bytes.drain(..extra);
        drop(bytes);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Tail {
    /// The kept bytes as text, without the half of a character the cut may have left.
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let start = bytes
            .iter()
            .position(|b| b & 0b1100_0000 != 0b1000_0000)
            .unwrap_or(bytes.len());
        String::from_utf8_lossy(bytes.get(start..).unwrap_or_default()).into_owned()
    }
}

/// Whether `e` is the link to the machine failing, so the command's fate is unknown: the driver's
/// transport error, or a machine that is not linked.
fn link_cut(e: &CommandError) -> bool {
    match e.error.downcast_ref::<DriverError>() {
        Some(DriverError::Transport { .. }) => true,
        Some(DriverError::Unavailable(why)) => why.contains("not linked"),
        _ => e.error.to_string().contains("is not linked"),
    }
}

/// The history line for a check that ended as `ran` says. An exit code or the deadline is a
/// result, and a cut link is a lost stream to run again. Every other end is a check that did not
/// pass, whose reason the line keeps.
fn outcome_line(job: &CheckJob, ran: Result<ExecView, &CommandError>, tail: &Tail) -> HistoryEntry {
    let (step, try_n) = (job.at.0.clone(), job.at.1);
    let not_run = |why: String| HistoryEntry::Checked {
        step: step.clone(),
        try_n,
        exit: NOT_RUN_EXIT,
        tail: format!("{}\n[moor: {why}]", tail.text()),
    };
    match ran {
        Err(e) if link_cut(e) => HistoryEntry::CheckLost { step, try_n },
        Err(e) => not_run(format!("the check did not run to a result: {}", e.error)),
        Ok(ExecView {
            exit: None,
            timed_out: false,
        }) => not_run("the check was interrupted before it exited".to_owned()),
        Ok(ExecView {
            exit: Some(exit), ..
        }) => HistoryEntry::Checked {
            step,
            try_n,
            exit,
            tail: tail.text(),
        },
        Ok(ExecView {
            timed_out: true, ..
        }) => HistoryEntry::Checked {
            step,
            try_n,
            exit: DEADLINE_EXIT,
            tail: format!(
                "{}\n[moor stopped the check at its deadline of {} s]",
                tail.text(),
                job.timeout_secs
            ),
        },
    }
}

/// Runs the check of `job` in its Box as the Box's owner and records how it ended. Returns what
/// to log.
pub(crate) fn run_check(cx: &Cx, job: &CheckJob) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let Ok(Some(row)) = store.box_row(job.box_id) else {
        return out;
    };
    if matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected) {
        return out;
    }
    let req = ExecReq {
        target: BoxTarget::Id(job.box_id),
        argv: vec![job.command.clone()],
        timeout_secs: Some(job.timeout_secs),
    };
    let tail = Tail::default();
    let (mut stdout, mut stderr) = (tail.clone(), tail.clone());
    let ran = cx.for_box(row.user, || {
        exec::run(
            cx,
            &req,
            Pipes {
                stdin: Box::new(io::empty()),
                stdout: &mut stdout,
                stderr: &mut stderr,
                stop: &|| false,
            },
        )
    });
    if let Err(e) = &ran {
        out.push(format!(
            "workflow check {}: no result from Box {}: {}",
            job.at.0,
            job.box_id.get(),
            e.error
        ));
    }
    let line = outcome_line(job, ran.as_ref().map(|v| *v), &tail);
    let (from, key) = (&job.at, (job.workflow, &job.repo, job.due_at));
    let at = ((&from.0, from.1), (&from.0, from.1));
    if let Err(e) = store.step_workflow_run(key, at, (&[line], cx.clock().now())) {
        out.push(format!("workflow check {}: {e}", job.at.0));
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

#[cfg(test)]
mod tests {
    use moor_core::seam::driver::DriverError;
    use moor_core::ErrorKind;

    use super::{link_cut, CommandError};

    fn failed(why: &str) -> CommandError {
        CommandError::new(ErrorKind::Failed, why.to_owned())
    }

    #[test]
    fn only_a_cut_link_is_lost() {
        let transport = DriverError::Transport {
            stderr_tail: String::new(),
        };
        assert!(link_cut(&CommandError::new(ErrorKind::Failed, transport)));
        assert!(link_cut(&failed(
            "box provider unavailable: machine 1 is not linked"
        )));
        for why in [
            "the Proxy could not start: no port; retry once it starts",
            "Box 3 is busy with another command; waited 30s",
            "store: database is locked",
            "no open Box 3",
        ] {
            assert!(!link_cut(&failed(why)), "{why}");
        }
        let timeout = DriverError::Timeout(std::time::Duration::from_secs(1));
        assert!(!link_cut(&CommandError::new(ErrorKind::Failed, timeout)));
    }
}
