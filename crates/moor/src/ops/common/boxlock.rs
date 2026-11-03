//! Per-Box advisory lock held across provisioning and other long Box operations.
//!
//! The kernel drops the flock when the holder dies, so stale lock files never block.

use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moor_core::name::BoxId;
use moor_core::seam::clock::Clock;
use moor_core::ErrorKind;

use crate::cx::Cancel;
use crate::error::CommandError;

/// How long an op waits for another holder before refusing as busy.
pub(crate) const WAIT: Duration = Duration::from_secs(10);

/// Pause between attempts while another holder owns the lock.
const POLL: Duration = Duration::from_millis(50);

/// The holds a forced close may interrupt in this process, by lock file.
static INTERRUPTIBLE: Mutex<BTreeMap<PathBuf, Interruptible>> = Mutex::new(BTreeMap::new());

/// An interruptible hold: the flag that stops its holder, and whoever waits for it to end.
#[derive(Debug)]
struct Interruptible {
    /// The holder's stop flag.
    cancel: Cancel,
    /// Hang up once the hold ends.
    waiters: Vec<Sender<()>>,
}

/// The interruptible holds, whichever thread poisoned them.
fn interruptible() -> MutexGuard<'static, BTreeMap<PathBuf, Interruptible>> {
    INTERRUPTIBLE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `<state>/locks/<id>.box.lock`.
fn lock_path(state: &Path, box_id: BoxId) -> PathBuf {
    state
        .join("locks")
        .join(format!("{}.box.lock", box_id.get()))
}

/// Exclusive hold on one Box; released on drop.
#[derive(Debug)]
pub(crate) struct BoxLock {
    /// Open file carrying the flock.
    file: File,
    /// Where the lock file is, naming the hold in [`INTERRUPTIBLE`].
    path: PathBuf,
}

impl BoxLock {
    /// Take `<state>/locks/<id>.box.lock`, polling until `wait` has passed on `clock`.
    ///
    /// # Errors
    /// `Refused("box busy: <holder-op> in progress")` when still held after `wait`;
    /// `Failed` when the lock file cannot be opened.
    pub(crate) fn acquire(
        clock: &dyn Clock,
        state: &Path,
        box_id: BoxId,
        op: &str,
        wait: Duration,
    ) -> Result<Self, CommandError> {
        let path = lock_path(state, box_id);
        let dir = state.join("locks");
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
            match file.try_lock() {
                Ok(()) => {
                    file.set_len(0)
                        .and_then(|()| file.write_all(op.as_bytes()))
                        .map_err(failed)?;
                    return Ok(Self { file, path });
                }
                Err(TryLockError::WouldBlock) if clock.instant() < deadline => clock.sleep(POLL),
                Err(TryLockError::WouldBlock) => {
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
                Err(TryLockError::Error(e)) => return Err(failed(e)),
            }
        }
    }

    /// This hold, which [`interrupt`] stops by setting `cancel`.
    pub(crate) fn interruptible(self, cancel: &Cancel) -> Self {
        let hold = Interruptible {
            cancel: cancel.clone(),
            waiters: Vec::new(),
        };
        interruptible().insert(self.path.clone(), hold);
        self
    }
}

impl Drop for BoxLock {
    fn drop(&mut self) {
        // Unlocked first, so whoever hears the hold end can take the lock straight away.
        let _ = self.file.unlock();
        interruptible().remove(&self.path);
    }
}

/// Stops the interruptible hold on `box_id`, if there is one; the receiver hangs up once it ends.
pub(crate) fn interrupt(state: &Path, box_id: BoxId) -> Option<Receiver<()>> {
    let mut holds = interruptible();
    let hold = holds.get_mut(&lock_path(state, box_id))?;
    hold.cancel.cancel();
    let (tx, rx) = mpsc::channel();
    hold.waiters.push(tx);
    drop(holds);
    Some(rx)
}

/// Whether the hold on `box_id` stops for [`interrupt`].
pub(crate) fn is_interruptible(state: &Path, box_id: BoxId) -> bool {
    interruptible().contains_key(&lock_path(state, box_id))
}

/// Waits until `wait` has passed on `clock` for the interrupted hold `ended` to end; whether it did.
///
/// The wait blocks on the hold's end, so it returns as soon as the holder lets go.
pub(crate) fn wait_ended(clock: &dyn Clock, ended: &Receiver<()>, wait: Duration) -> bool {
    let deadline = clock.instant() + wait;
    loop {
        if ended.recv_timeout(POLL) != Err(RecvTimeoutError::Timeout) {
            return true;
        }
        if clock.instant() >= deadline {
            return false;
        }
    }
}

#[cfg(test)]
mod tests;
