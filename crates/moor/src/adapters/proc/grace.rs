//! How a child is stopped: SIGTERM, a tool-specific grace, then SIGKILL.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use moor_core::seam::proc::Tool;
use rustix::process::Signal;

use super::signal::signal_group;
use super::{gone, Waited, POLL};

/// Longest wait between SIGTERM and SIGKILL.
pub(super) const GRACE: Duration = Duration::from_secs(2);
/// Crabbox releases its remote owner over ssh for up to 30s after SIGTERM.
const CRABBOX_GRACE: Duration = Duration::from_secs(35);

/// Wait between SIGTERM and SIGKILL: crabbox gets [`CRABBOX_GRACE`], others [`GRACE`] capped by the deadline.
pub(super) fn grace_for(tool: Tool, deadline: Duration) -> Duration {
    match tool {
        Tool::Crabbox => CRABBOX_GRACE,
        _ => GRACE.min(deadline.max(POLL)),
    }
}

/// Interactive second-interrupt escape for [`terminate_with`].
pub(super) struct Abort<'a> {
    /// Set by another interrupt; cleared when it cuts the grace short.
    pub flag: &'a AtomicBool,
    /// Receives the one-line notice that crabbox is being waited for.
    pub notice: &'a mut dyn Write,
}

/// Slice of the grace wait between looks at [`Abort::flag`].
const ABORT_POLL: Duration = Duration::from_millis(50);

/// SIGTERM the group, wait the tool's grace, SIGKILL the group, reap the leader.
pub(super) fn terminate(
    pid: u32,
    rx: &Receiver<Waited>,
    tool: Tool,
    deadline: Duration,
) -> io::Result<()> {
    terminate_with(pid, rx, tool, deadline, None)
}

/// As [`terminate`]; with `abort`, crabbox's wait is announced once and a set flag ends it early.
pub(super) fn terminate_with(
    pid: u32,
    rx: &Receiver<Waited>,
    tool: Tool,
    deadline: Duration,
    abort: Option<Abort<'_>>,
) -> io::Result<()> {
    signal_group(pid, Signal::TERM);
    let grace = grace_for(tool, deadline);
    let received = match abort {
        Some(a) if tool == Tool::Crabbox => {
            drop(writeln!(
                a.notice,
                "waiting for crabbox to release the Box workspace (Ctrl-C again to force)"
            ));
            drop(a.notice.flush());
            wait_or_abort(rx, grace, a.flag)
        }
        _ => rx.recv_timeout(grace),
    };
    let exited = match received {
        Ok(st) => {
            st?;
            true
        }
        Err(RecvTimeoutError::Timeout) => false,
        Err(RecvTimeoutError::Disconnected) => return Err(gone()),
    };
    signal_group(pid, Signal::KILL);
    if !exited {
        rx.recv().map_err(|_| gone())??;
    }
    Ok(())
}

/// Waits up to `grace` for the child, returning early as a timeout once `flag` is set.
fn wait_or_abort(
    rx: &Receiver<Waited>,
    grace: Duration,
    flag: &AtomicBool,
) -> Result<Waited, RecvTimeoutError> {
    let mut left = grace;
    while !left.is_zero() {
        let slice = left.min(ABORT_POLL);
        match rx.recv_timeout(slice) {
            Err(RecvTimeoutError::Timeout) => {
                left = left.saturating_sub(slice);
                if flag.swap(false, Ordering::SeqCst) {
                    break;
                }
            }
            other => return other,
        }
    }
    Err(RecvTimeoutError::Timeout)
}
