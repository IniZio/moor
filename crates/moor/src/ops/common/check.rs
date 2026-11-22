//! Running a command in a Box as a check, for the workflow tick and the gate pass alike: the
//! command goes through the exec op in the Box's work place, and its end is read as an exit code or
//! as a lost stream.
//!
//! The act comes first and its line after, so a controller that stops in between runs the check
//! again. A cut link is no result: it is a lost stream, and the caller reruns it. Any other error
//! before an exit code, such as a busy Box, is a check that did not run.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::request::BoxTarget;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::workflow::RunId;
use moor_core::{BoxId, ExecReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::exec;

/// The longest a check may run, however long the run has left.
pub(crate) const CHECK_LIMIT: Duration = Duration::from_mins(30);

/// How much of a check's output is kept: its end.
pub(crate) const TAIL_BYTES: usize = 4096;

/// What is recorded for a check stopped at its deadline, which has no exit code.
const DEADLINE_EXIT: i32 = -1;

/// What is recorded for a check that a signal stopped before it exited, and what the workflow tick
/// records for a check that did not run.
pub(crate) const NOT_RUN_EXIT: i32 = -2;

/// What a check in flight is claimed by: a workflow run, or a Box with gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Claim {
    /// A workflow run, which has one check at a time.
    Run(RunId),
    /// A Box with gates, which has one check at a time.
    Box(BoxId),
}

/// The checks running in this process, by state directory and claim, so two ticks never run one
/// check at once. A controller restart forgets them, and the check then runs again.
fn flights() -> &'static Mutex<HashSet<(PathBuf, Claim)>> {
    static FLIGHTS: OnceLock<Mutex<HashSet<(PathBuf, Claim)>>> = OnceLock::new();
    FLIGHTS.get_or_init(Mutex::default)
}

/// A claim whose check is running; dropping it ends the claim.
pub(crate) struct Flight(PathBuf, Claim);

impl Flight {
    /// Claims `claim`; none when its check is already running.
    pub(crate) fn claim(cx: &Cx, claim: Claim) -> Option<Self> {
        let dir = cx.state_dir().map(PathBuf::from).unwrap_or_default();
        let fresh = flights()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((dir.clone(), claim));
        fresh.then_some(Self(dir, claim))
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

/// How a check ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    /// It ran to a result: an exit code, or the deadline, or a signal, each with the end of its
    /// output.
    Exit {
        /// The exit code.
        exit: i32,
        /// The end of its output.
        tail: String,
    },
    /// The link was cut before an exit code: run it again.
    Lost,
    /// The exec failed before the check ran to a result, such as a busy Box or a Proxy that could
    /// not start; the tail says why. No result, so it decides nothing.
    NotRun {
        /// The end of its output, with why it did not run.
        tail: String,
    },
}

/// A check that ran, and the error that cut it short, when one did.
pub(crate) struct Ran {
    /// How it ended.
    pub(crate) ended: Ended,
    /// The error the exec gave, for the log.
    pub(crate) error: Option<String>,
}

/// How a check that ended as `ran` says, with the `timeout_secs` it was held to. An exit code,
/// the deadline or a signal is a result, a cut link is a lost stream, and any other error is a
/// check that did not run.
fn ended_by(ran: Result<ExecView, &CommandError>, tail: &Tail, timeout_secs: u32) -> Ended {
    let said = |why: String| format!("{}\n[moor: {why}]", tail.text());
    match ran {
        Err(e) if link_cut(e) => Ended::Lost,
        Err(e) => Ended::NotRun {
            tail: said(format!("the check did not run to a result: {}", e.error)),
        },
        Ok(ExecView {
            exit: None,
            timed_out: false,
        }) => Ended::Exit {
            exit: NOT_RUN_EXIT,
            tail: said("the check was interrupted before it exited".to_owned()),
        },
        Ok(ExecView {
            exit: Some(exit), ..
        }) => Ended::Exit {
            exit,
            tail: tail.text(),
        },
        Ok(ExecView {
            timed_out: true, ..
        }) => Ended::Exit {
            exit: DEADLINE_EXIT,
            tail: format!(
                "{}\n[moor stopped the check at its deadline of {timeout_secs} s]",
                tail.text()
            ),
        },
    }
}

/// Runs `command` in Box `box_id` as the Box's owner, held to `timeout_secs`; none when the Box
/// is gone or closed, which leaves nothing to record.
pub(crate) fn execute(cx: &Cx, box_id: BoxId, command: &str, timeout_secs: u32) -> Option<Ran> {
    let store = cx.store().ok()?;
    let row = store.box_row(box_id).ok()??;
    if matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected) {
        return None;
    }
    let req = ExecReq {
        target: BoxTarget::Id(box_id),
        argv: vec![command.to_owned()],
        timeout_secs: Some(timeout_secs),
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
    Some(Ran {
        error: ran.as_ref().err().map(|e| e.error.to_string()),
        ended: ended_by(ran.as_ref().map(|v| *v), &tail, timeout_secs),
    })
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
