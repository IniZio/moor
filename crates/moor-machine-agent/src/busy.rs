//! The work a restart would cut: every stream the runner opened, on any link, and every boot or
//! fork going on. A restart in place waits until there is none.

use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

/// How much work is going on.
static GOING: Mutex<usize> = Mutex::new(0);
/// Woken each time the last of it ends.
static IDLE: Notify = Notify::const_new();

/// The work count, whichever thread poisoned it.
fn going() -> MutexGuard<'static, usize> {
    GOING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One piece of work, counted until it is dropped.
pub(crate) struct Busy(());

/// Counts a piece of work until the returned guard drops.
pub(crate) fn busy() -> Busy {
    *going() += 1;
    Busy(())
}

impl Drop for Busy {
    fn drop(&mut self) {
        let mut going = going();
        *going = going.saturating_sub(1);
        if *going == 0 {
            IDLE.notify_waiters();
        }
    }
}

/// Runs `then` once no work is going on, holding off any new work while it runs, so that a
/// restart in `then` cuts nothing.
pub(crate) async fn when_idle<T>(then: impl FnOnce() -> T) -> T {
    loop {
        let woken = IDLE.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        {
            let going = going();
            if *going == 0 {
                return then();
            }
        }
        woken.await;
    }
}
