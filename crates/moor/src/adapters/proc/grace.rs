//! How a child is stopped: SIGTERM, a grace, then SIGKILL.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use rustix::process::Signal;

use super::signal::signal_group;
use super::{gone, Waited, POLL};

/// Longest wait between SIGTERM and SIGKILL.
pub(super) const GRACE: Duration = Duration::from_secs(2);

/// Wait between SIGTERM and SIGKILL: [`GRACE`] capped by the deadline.
pub(super) fn grace_for(deadline: Duration) -> Duration {
    GRACE.min(deadline.max(POLL))
}

/// SIGTERM the group, wait the grace, SIGKILL the group, reap the leader.
pub(super) fn terminate(pid: u32, rx: &Receiver<Waited>, deadline: Duration) -> io::Result<()> {
    signal_group(pid, Signal::TERM);
    let exited = match rx.recv_timeout(grace_for(deadline)) {
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
