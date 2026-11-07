//! The one-controller rule on a database with no file lock to share: a controller holds a lock
//! there from before it migrates until its process exits, and leaves at once if it loses it.
//!
//! A store in a file needs none of this, as the file lock beside it already serves.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::db::{ControllerLock, Target};
use super::{Database, StoreError};

/// Time between rewrites of the file a liveness probe reads.
const ALIVE_EVERY: Duration = Duration::from_secs(5);

/// How often a waiter looks up from the lock to see whether it was told to stop.
const LOOK_UP: Duration = Duration::from_millis(200);

/// Called once if a holder loses the lock, with why.
pub(crate) type Lost = Arc<dyn Fn(&str) + Send + Sync>;

/// A controller wanting the lock, and how it hears the world while waiting.
pub(crate) struct Claimant<'a> {
    /// The name its session gives.
    pub(crate) me: String,
    /// The file whose age a liveness probe reads.
    pub(crate) alive: PathBuf,
    /// Waits up to its argument; true once the controller was told to stop.
    pub(crate) stopped: &'a dyn Fn(Duration) -> bool,
    /// Reports a line about the wait.
    pub(crate) note: &'a dyn Fn(&str),
    /// Called if a holder loses the lock.
    pub(crate) lost: Lost,
}

/// What a claim ended in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Claim {
    /// The controller may run until its process exits, which frees the lock.
    Held,
    /// It was told to stop while waiting.
    Stopped,
}

/// What the thread on the lock's session reports.
enum Step {
    /// It is waiting behind another holder.
    Waiting(String),
    /// It holds the lock.
    Held,
    /// It could not take the lock.
    Failed(StoreError),
}

impl Claimant<'_> {
    /// Waits for the lock on `database`. A store in a file needs no claim.
    pub(crate) fn claim(self, database: Option<&Database>) -> Result<Claim, StoreError> {
        let Some(database) = database else {
            return Ok(Claim::Held);
        };
        let target = Target::parse(&database.url).map_err(StoreError::Url)?;
        self.claim_on(&target)
    }

    /// Waits for the lock on `target`, on one connection.
    pub(crate) fn claim_on(self, target: &Target) -> Result<Claim, StoreError> {
        spawn_alive(self.alive);
        let (steps, reports) = mpsc::channel();
        let (target, me, lost) = (target.clone(), self.me, self.lost);
        thread::spawn(move || hold(&target, &me, &steps, &lost));
        loop {
            match reports.try_recv() {
                Ok(Step::Waiting(line)) => (self.note)(&line),
                Ok(Step::Held) => return Ok(Claim::Held),
                Ok(Step::Failed(e)) => return Err(e),
                Err(TryRecvError::Disconnected) => return Ok(Claim::Stopped),
                Err(TryRecvError::Empty) if (self.stopped)(LOOK_UP) => return Ok(Claim::Stopped),
                Err(TryRecvError::Empty) => {}
            }
        }
    }
}

/// Takes the lock on `target`, reports each step, then blocks on its session until that ends.
fn hold(target: &Target, me: &str, steps: &Sender<Step>, lost: &Lost) {
    match take(target, me, steps) {
        Ok(lock) => {
            drop(steps.send(Step::Held));
            lost(&lock.until_lost());
        }
        Err(e) => drop(steps.send(Step::Failed(e))),
    }
}

/// A session on `target` holding the lock, once that is possible.
fn take(target: &Target, me: &str, steps: &Sender<Step>) -> Result<ControllerLock, StoreError> {
    let mut lock = ControllerLock::connect(target, me)?;
    if !lock.try_take()? {
        let holder = lock.holder()?.unwrap_or_else(|| "?".to_owned());
        let line = format!("waiting for the controller lock held by {holder}");
        drop(steps.send(Step::Waiting(line)));
        lock.take()?;
    }
    Ok(lock)
}

/// Rewrites `alive` from its own thread until the process exits, so its age says the process has
/// stopped running. A failed write leaves the file ageing, so the probe reports that too.
fn spawn_alive(alive: PathBuf) {
    touch(&alive);
    thread::spawn(move || loop {
        thread::sleep(ALIVE_EVERY);
        touch(&alive);
    });
}

/// Marks this process alive.
fn touch(alive: &Path) {
    drop(std::fs::write(alive, std::process::id().to_string()));
}

#[cfg(test)]
mod tests;
