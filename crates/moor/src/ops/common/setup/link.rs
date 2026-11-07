//! What a setup does while its Box cannot be reached, as a machine between two runners.

use std::path::Path;
use std::time::{Duration, Instant};

use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, ErrorKind, Warning};

use super::attach::REFOLLOW_AFTER;
use super::{Failure, SetupRun};
use crate::cx::Cx;
use crate::error::CommandError;

/// How long a follower keeps attaching while the Box cannot be reached, which a machine's link
/// being down between two runners looks like; the run in the Box goes on meanwhile.
pub(crate) const LINK_WAIT: Duration = Duration::from_secs(120);

/// Longest one call that asks whether the Box answers may take.
const REACHABLE_CALL: Duration = Duration::from_secs(10);

/// This process stopped following a setup it left to the next one: the run goes on in the Box,
/// and nothing about it is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Released;

impl From<Released> for CommandError {
    fn from(_: Released) -> Self {
        Self::new(
            ErrorKind::Failed,
            "the controller is restarting; setup goes on in the Box and the next controller follows it",
        )
    }
}

/// Why a follower stopped following a run that may still be going.
pub(super) enum Left {
    /// The Box stayed unreachable for the whole wait, with what ssh last said.
    LinkLost(String),
    /// A drain began while the Box could not be reached, so the run is left to the next process.
    Released,
}

/// Waits until the Box answers, or [`LINK_WAIT`] passes and the steps' own calls report
/// why, so a process that follows a setup on start does not fail it before the machine relinks.
///
/// # Errors
/// A drain began, so the setup is left to the next process.
pub(crate) fn await_reachable(cx: &Cx, handle: &BoxHandle) -> Result<(), Released> {
    let spec = ExecSpec {
        argv: vec!["true".to_owned()],
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let began = cx.clock().instant();
    while let Err(DriverError::Transport { .. }) =
        cx.driver().exec(handle, &spec, Deadline(REACHABLE_CALL))
    {
        if cx.env().draining().is_cancelled() {
            return Err(Released);
        }
        if cx.env().cancel().is_cancelled() {
            break;
        }
        let waited = cx.clock().instant().saturating_duration_since(began);
        if waited >= LINK_WAIT {
            break;
        }
        cx.clock().sleep(REFOLLOW_AFTER);
    }
    Ok(())
}

/// A setup the Box could not be reached for: no line is blamed, and the reason is what the Box's
/// event shows.
pub(super) fn link_lost(cx: &Cx, reason: &str, ms: u64) -> SetupRun {
    let line = format!(
        "the Box stayed unreachable for {} s, so no setup line is known to have failed: {reason}",
        LINK_WAIT.as_secs()
    );
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: line.clone(),
    });
    SetupRun::Failed {
        line,
        exit: None,
        ms,
        tail: String::new(),
    }
}

/// Whether a follower that has not reached the Box `since` attaches again; `left` is what remains
/// of the run's deadline.
pub(super) fn keep_waiting(
    cx: &Cx,
    since: Instant,
    reason: &str,
    left: Duration,
) -> Result<(), Left> {
    if cx.env().draining().is_cancelled() {
        return Err(Left::Released);
    }
    let waited = cx.clock().instant().saturating_duration_since(since);
    if left.is_zero() || waited >= LINK_WAIT {
        return Err(Left::LinkLost(reason.to_owned()));
    }
    Ok(())
}

/// What ssh said when `result` is a connection to the Box that failed or dropped.
pub(super) fn unreachable(result: &Result<ExecOutcome, Failure>) -> Option<&str> {
    match result {
        Err(Failure::Exec(DriverError::Transport { stderr_tail })) => Some(stderr_tail),
        _ => None,
    }
}

/// Whether the attach that just ended read any of the run's log, so the Box was reachable.
pub(super) fn reached_the_box(output: &Path) -> bool {
    std::fs::metadata(output).is_ok_and(|m| m.len() > 0)
}
