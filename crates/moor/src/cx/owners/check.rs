//! The test-build check that every write to a Box's rows happens under its owner, and test holds.

#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use moor_core::name::BoxId;

#[cfg(test)]
use super::{Cmd, Owners};
#[cfg(test)]
use crate::env::Cancel;

#[cfg(test)]
thread_local! {
    /// Whether this thread runs an op whose Box writes are checked against their owners.
    static CHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f`, an op entry, with every write it makes to a Box's rows on this thread checked, in
/// test builds, against that Box's owner. Writes from a thread holding a turn are always checked;
/// test fixtures that seed rows outside any op are not.
pub(crate) fn checked<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    let was = CHECKED.replace(true);
    let out = f();
    #[cfg(test)]
    CHECKED.set(was);
    out
}

#[cfg(test)]
impl Owners {
    /// Panics unless something holds `box_id`'s owner: a turn, a waiter about to take it, or a
    /// run. Per Box, not per thread, as a setup follower writes under its caller's turn.
    pub(crate) fn check_write(&self, box_id: BoxId) {
        let (armed, owned) = {
            let st = self.lock();
            (
                CHECKED.get() || st.held_here().is_some(),
                st.boxes.contains_key(&box_id),
            )
        };
        assert!(
            !armed || owned,
            "Box {} written with no turn held",
            box_id.get()
        );
    }

    /// Holds `box_id`'s turn for `cmd` on another thread until the returned hold drops, as an op
    /// in flight elsewhere would.
    pub(crate) fn hold(self: &Arc<Self>, box_id: BoxId, cmd: Cmd) -> Held {
        self.hold_with(box_id, cmd, Cancel::default())
    }

    /// [`Owners::hold`] with `cancel` as the holder's stop flag, which a forced close sets; the
    /// hold still lasts until it drops.
    pub(crate) fn hold_with(self: &Arc<Self>, box_id: BoxId, cmd: Cmd, cancel: Cancel) -> Held {
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let owners = Arc::clone(self);
        let thread = thread::spawn(move || {
            let deadline = owners.clock.instant() + Duration::from_secs(600);
            let turn = owners.turn(box_id, cmd, &cancel, deadline);
            let ok = turn.is_ok();
            let _ = held_tx.send(turn.as_ref().map(drop).map_err(|e| e.error.to_string()));
            if ok {
                let _ = released.recv();
            }
            drop(turn);
        });
        let held = held_rx
            .recv()
            .unwrap_or_else(|_| Err("holder died".to_owned()));
        assert!(
            held.is_ok(),
            "could not hold Box {}: {held:?}",
            box_id.get()
        );
        Held {
            release: Some(release),
            thread: Some(thread),
        }
    }

    /// Owners currently held in memory.
    pub(super) fn owned(&self) -> usize {
        self.lock().boxes.len()
    }

    /// Ops waiting for `box_id`'s turn.
    pub(crate) fn waiting(&self, box_id: BoxId) -> usize {
        self.lock().boxes.get(&box_id).map_or(0, |o| o.queue.len())
    }
}

/// A turn held on another thread by [`Owners::hold`]; released on drop.
#[cfg(test)]
pub(crate) struct Held {
    /// Hung up to release the turn.
    release: Option<std::sync::mpsc::Sender<()>>,
    /// The thread holding the turn.
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(test)]
impl Drop for Held {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
