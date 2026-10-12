//! Per-Box advisory lock held across provisioning and other long Box operations.
//!
//! The kernel drops the flock when the holder dies, so stale lock files never block.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use moor_core::name::TaskId;
use moor_core::seam::clock::Clock;
use moor_core::ErrorKind;
use rustix::fs::{flock, FlockOperation};
use rustix::io::Errno;

use crate::error::CommandError;

/// Pause between attempts while another holder owns the lock.
const POLL: Duration = Duration::from_millis(50);

/// Exclusive hold on one Box; released on drop.
#[derive(Debug)]
#[cfg_attr(not(test), expect(dead_code, reason = "taken by start and close"))]
pub(crate) struct BoxLock {
    /// Open file carrying the flock.
    _file: File,
}

impl BoxLock {
    /// Take `<state>/locks/<task>.box.lock`, polling until `wait` has passed on `clock`.
    ///
    /// # Errors
    /// `Refused("box busy: <holder-op> in progress")` when still held after `wait`;
    /// `Failed` when the lock file cannot be opened.
    #[cfg_attr(not(test), expect(dead_code, reason = "taken by start and close"))]
    pub(crate) fn acquire(
        clock: &dyn Clock,
        state: &Path,
        task: TaskId,
        op: &str,
        wait: Duration,
    ) -> Result<Self, CommandError> {
        let dir = state.join("locks");
        let path = dir.join(format!("{}.box.lock", task.get()));
        let failed = |e: std::io::Error| {
            CommandError::new(
                ErrorKind::Failed,
                format!("cannot open {}: {e}", path.display()),
            )
        };
        fs::create_dir_all(&dir).map_err(failed)?;
        let mut file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(failed)?;
        let deadline = clock.instant() + wait;
        loop {
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => {
                    file.set_len(0)
                        .and_then(|()| file.write_all(op.as_bytes()))
                        .map_err(failed)?;
                    return Ok(Self { _file: file });
                }
                Err(Errno::WOULDBLOCK) if clock.instant() < deadline => clock.sleep(POLL),
                Err(Errno::WOULDBLOCK) => {
                    let holder = fs::read_to_string(&path).unwrap_or_default();
                    let holder = match holder.trim() {
                        "" => "another operation",
                        name => name,
                    };
                    return Err(CommandError::new(
                        ErrorKind::Refused,
                        format!("box busy: {holder} in progress"),
                    ));
                }
                Err(e) => return Err(failed(e.into())),
            }
        }
    }
}

#[cfg(test)]
mod tests;
