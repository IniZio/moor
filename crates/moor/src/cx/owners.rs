//! One owner per Box admits every op on it: each op runs under that Box's turn.
//!
//! An owner lives in memory only while it has a holder, waiters or a run in flight. The holder's
//! op runs on the caller's own thread, so admission, refusal and preemption answer at once.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use moor_core::name::BoxId;
use moor_core::seam::clock::Clock;

use self::policy::Admit;
pub(crate) use self::policy::Cmd;
use crate::env::Cancel;
use crate::error::CommandError;

mod check;
mod policy;
mod state;
mod turn;

pub(crate) use self::check::checked;
#[cfg(test)]
pub(crate) use self::check::Held;
use self::state::{Grant, Holder, State};
#[cfg(test)]
pub(crate) use self::turn::RunHold;
pub(crate) use self::turn::{Turn, TurnShare};

/// What ends on a Box: the turn granted to a ticket, or the run a thread began.
#[derive(Clone, Copy)]
enum Released {
    /// The turn of this ticket.
    Turn(u64),
    /// The run this thread began.
    Run(std::thread::ThreadId),
}

/// How long a forced close waits for the start it cancelled to let go.
pub(crate) const INTERRUPT_WAIT: Duration = Duration::from_secs(30);

/// The refusal for a forced close whose cancelled `holder` kept the turn past [`INTERRUPT_WAIT`].
fn not_stopped(holder: Cmd) -> CommandError {
    let why = format!("box busy: {} did not stop after cancel", holder.name());
    CommandError::new(moor_core::ErrorKind::Refused, why)
}

/// Longest a waiter sleeps before it looks at its own cancel flag and deadline again.
const POLL: Duration = Duration::from_millis(50);

/// How long a waiter on a clock that does not move on its own waits for each [`POLL`] it moves it.
const TICK: Duration = Duration::from_millis(1);

/// The owners of every Box with an op in flight.
pub(crate) struct Owners {
    /// Every owner.
    state: Mutex<State>,
    /// Signalled whenever a turn, queue or run changes.
    changed: Condvar,
    /// What waiters measure their deadline on.
    clock: Arc<dyn Clock>,
}

impl Owners {
    /// No owners yet; waiters read time from `clock`.
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Mutex::default(),
            changed: Condvar::new(),
            clock,
        }
    }

    /// The state, whichever thread poisoned it.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` under `box_id`'s turn on this thread, once [`policy::admit`] lets `cmd` in, and returns
    /// what `f` returns.
    ///
    /// # Errors
    /// As [`Owners::turn`], else `f`'s own result.
    #[cfg(test)]
    pub(crate) fn call<'a, T>(
        &'a self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        deadline: Instant,
        f: impl FnOnce(&Turn<'a>) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let turn = self.turn(box_id, cmd, cancel, deadline)?;
        f(&turn)
    }

    /// `box_id`'s turn for `cmd`, once [`policy::admit`] lets it in; released when the turn drops.
    ///
    /// Waiters are served in arrival order. A forced close of a long holder cancels it and goes
    /// first; one the holder still blocks after [`INTERRUPT_WAIT`] is refused, never run beside it.
    ///
    /// # Errors
    /// `Refused` when the policy refuses `cmd`, when [`state::MAX_QUEUE`] ops already wait, when
    /// `deadline` passes or `cancel` is set while waiting, or on [`Owners::drain`]; `Internal`
    /// when this thread already holds a turn.
    pub(crate) fn turn(
        &self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        deadline: Instant,
    ) -> Result<Turn<'_>, CommandError> {
        self.preempting(box_id, cmd, cancel, deadline, || Ok(()))
            .map(|(turn, _)| turn)
    }

    /// [`Owners::turn`], running `before` first when `cmd` preempts the holder, and saying whether
    /// it did. `before` runs after the call is queued at the head and before the holder is
    /// cancelled.
    ///
    /// # Errors
    /// As [`Owners::turn`], or `before`'s error, which leaves the holder running.
    pub(crate) fn preempting(
        &self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        deadline: Instant,
        before: impl FnOnce() -> Result<(), CommandError>,
    ) -> Result<(Turn<'_>, bool), CommandError> {
        let (seq, admitted) = self.lock().enter(box_id, cmd)?;
        let preempts = admitted == Admit::Preempt;
        if preempts {
            if let Err(e) = before() {
                self.lock().leave(box_id, seq);
                self.changed.notify_all();
                return Err(e);
            }
        }
        let turn = self.wait_turn(box_id, (seq, cmd), cancel, deadline)?;
        Ok((turn, preempts))
    }

    /// Runs `f` under `box_id`'s turn only when nothing is in flight or waiting on it; `None`
    /// when the Box is busy, so a background pass skips it.
    #[cfg(test)]
    pub(crate) fn try_call<'a, T>(
        &'a self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        f: impl FnOnce(&Turn<'a>) -> T,
    ) -> Option<T> {
        let turn = self.try_turn(box_id, cmd, cancel)?;
        Some(f(&turn))
    }

    /// `box_id`'s turn when nothing is in flight or waiting on it; `None` when it is busy. The
    /// only way to reach another Box from inside a turn, as it never waits.
    pub(crate) fn try_turn(&self, box_id: BoxId, cmd: Cmd, cancel: &Cancel) -> Option<Turn<'_>> {
        let mut st = self.lock();
        if st.draining || st.boxes.contains_key(&box_id) {
            return None;
        }
        let seq = st.next;
        st.next += 1;
        st.boxes.entry(box_id).or_default().holder = Some(Holder::new(seq, cmd, cancel));
        drop(st);
        Some(Turn::new(self, box_id, seq))
    }

    /// Whether this thread holds `box_id`'s turn.
    pub(crate) fn holds(&self, box_id: BoxId) -> bool {
        let me = std::thread::current().id();
        let st = self.lock();
        let held = (st.boxes.get(&box_id))
            .and_then(|o| o.holder.as_ref())
            .is_some_and(|h| h.thread == me);
        drop(st);
        held
    }

    /// Whether an op on another thread holds `box_id`'s turn or a run on it.
    pub(crate) fn busy(&self, box_id: BoxId) -> bool {
        let me = std::thread::current().id();
        let st = self.lock();
        let busy = st.boxes.get(&box_id).is_some_and(|o| {
            o.holder.as_ref().is_some_and(|h| h.thread != me) || o.runs.iter().any(|t| *t != me)
        });
        drop(st);
        busy
    }

    /// A share of the turn this thread holds, for a helper thread to act under; `None` when it
    /// holds none.
    pub(crate) fn held_share(&self) -> Option<TurnShare<'_>> {
        let st = self.lock();
        let me = std::thread::current().id();
        let held = st.boxes.iter().find_map(|(id, o)| {
            o.holder
                .as_ref()
                .filter(|h| h.thread == me)
                .map(|h| (*id, h.seq))
        });
        drop(st);
        held.map(|(id, seq)| TurnShare::new(self, id, seq))
    }

    /// Wakes every waiter with a restarting error and refuses every later call; holders finish.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the controller drains its owners once boot resumes through them"
        )
    )]
    pub(crate) fn drain(&self) {
        self.lock().draining = true;
        self.changed.notify_all();
    }

    /// Blocks until ticket `seq`, queued for `cmd` on `box_id`, gets the turn, is refused or
    /// gives up.
    fn wait_turn(
        &self,
        box_id: BoxId,
        (seq, cmd): (u64, Cmd),
        cancel: &Cancel,
        deadline: Instant,
    ) -> Result<Turn<'_>, CommandError> {
        let mut st = self.lock();
        let mut frozen = false;
        let mut preempt_by = None;
        loop {
            let now = self.clock.instant();
            let err = match st.grant(box_id, seq, cmd, cancel) {
                Ok(Grant::Granted) => return Ok(Turn::new(self, box_id, seq)),
                Ok(Grant::Preempting(holder)) => {
                    let by = *preempt_by.get_or_insert(now + INTERRUPT_WAIT);
                    (now >= by).then(|| not_stopped(holder))
                }
                Ok(Grant::Wait) => None,
                Err(e) => Some(e),
            };
            let gave_up = cancel.is_cancelled() || now >= deadline;
            let err = err.or_else(|| gave_up.then(|| st.busy(box_id)));
            if let Some(err) = err {
                st.leave(box_id, seq);
                drop(st);
                self.changed.notify_all();
                return Err(err);
            }
            let before = self.clock.instant();
            let (next, waited) = self
                .changed
                .wait_timeout(st, if frozen { TICK } else { POLL })
                .unwrap_or_else(PoisonError::into_inner);
            st = next;
            // A clock that does not move on its own, as in tests, moves a poll each tick instead.
            if waited.timed_out() && self.clock.instant() == before {
                frozen = true;
                drop(st);
                self.clock.sleep(POLL);
                st = self.lock();
            }
        }
    }

    /// Releases `what` on `box_id` and wakes the waiters.
    fn release(&self, box_id: BoxId, what: Released) {
        let mut st = self.lock();
        if let Some(owner) = st.boxes.get_mut(&box_id) {
            match what {
                Released::Turn(seq) if owner.holder.as_ref().is_some_and(|h| h.seq == seq) => {
                    owner.holder = None;
                }
                Released::Turn(_) => {}
                Released::Run(thread) => {
                    if let Some(at) = owner.runs.iter().position(|t| *t == thread) {
                        owner.runs.swap_remove(at);
                    }
                }
            }
        }
        st.drop_idle(box_id);
        drop(st);
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests;
