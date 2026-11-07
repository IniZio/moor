//! The work a restart would cut: every stream the runner opened, on any link, and every boot or
//! fork going on. A restart in place waits until there is none.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;

/// The agent's work.
static WORK: Work = Work::new();

/// Work going on, each piece by what it is.
pub(crate) struct Work {
    /// Each piece by its number.
    going: Mutex<(u64, BTreeMap<u64, String>)>,
    /// Woken each time the last piece ends.
    idle: Notify,
}

/// One piece of the agent's work, counted until it is dropped.
pub(crate) struct Busy {
    /// Its number.
    n: u64,
    /// Where it is counted.
    work: &'static Work,
}

/// Counts a piece of the agent's work, as `what`, until the returned guard drops.
pub(crate) fn busy(what: &str) -> Busy {
    WORK.busy(what)
}

/// Runs `then` once no work is going on, holding off new work while it runs; after `patience`,
/// tells `stuck` once what still goes on.
pub(crate) async fn when_idle<T>(
    patience: Duration,
    stuck: impl FnOnce(&[String]),
    then: impl FnOnce() -> T,
) -> T {
    WORK.when_idle(patience, stuck, then).await
}

impl Work {
    /// No work.
    pub(crate) const fn new() -> Self {
        Self {
            going: Mutex::new((0, BTreeMap::new())),
            idle: Notify::const_new(),
        }
    }

    /// The pieces, whichever thread poisoned them.
    fn going(&self) -> MutexGuard<'_, (u64, BTreeMap<u64, String>)> {
        self.going.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts a piece of work, as `what`, until the returned guard drops.
    pub(crate) fn busy(&'static self, what: &str) -> Busy {
        let n = {
            let mut going = self.going();
            going.0 += 1;
            let n = going.0;
            going.1.insert(n, what.to_owned());
            n
        };
        Busy { n, work: self }
    }

    /// As [`when_idle`], for this work.
    pub(crate) async fn when_idle<T>(
        &self,
        patience: Duration,
        stuck: impl FnOnce(&[String]),
        then: impl FnOnce() -> T,
    ) -> T {
        let deadline = tokio::time::sleep(patience);
        tokio::pin!(deadline);
        let mut stuck = Some(stuck);
        loop {
            let woken = self.idle.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            {
                let going = self.going();
                if going.1.is_empty() {
                    return then();
                }
            }
            tokio::select! {
                () = woken => {}
                () = &mut deadline, if stuck.is_some() => {
                    let held: Vec<String> = self.going().1.values().cloned().collect();
                    if let Some(tell) = stuck.take() {
                        tell(&held);
                    }
                }
            }
        }
    }
}

impl Busy {
    /// Names this piece of work `what` from now on.
    pub(crate) fn name(&self, what: &str) {
        if let Some(name) = self.work.going().1.get_mut(&self.n) {
            what.clone_into(name);
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let mut going = self.work.going();
        going.1.remove(&self.n);
        if going.1.is_empty() {
            self.work.idle.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests;
