//! One owner per Box admits every op on it: each op runs under that Box's turn.
//!
//! An owner lives in memory only while it has a holder, waiters or a run in flight. The holder's
//! op runs on the caller's own thread, so admission, refusal and preemption answer at once.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use moor_core::name::BoxId;
use moor_core::seam::clock::Clock;
use moor_core::ErrorKind;

pub(crate) use self::policy::Cmd;
use self::policy::{admit, Admit};
use crate::cx::Cancel;
use crate::error::CommandError;

mod policy;

/// Most ops that may wait for one Box at once; past it a call is refused.
const MAX_QUEUE: usize = 16;

/// Longest a waiter sleeps before it looks at its own cancel flag and deadline again.
const POLL: Duration = Duration::from_millis(50);

/// Whoever has a Box's turn.
#[derive(Debug)]
struct Holder {
    /// The ticket the turn was granted to.
    seq: u64,
    /// What the holder is doing.
    cmd: Cmd,
    /// The holder's stop flag, which a forced close sets.
    cancel: Cancel,
    /// Calling the same Box again from this thread would wait on itself.
    thread: ThreadId,
}

impl Holder {
    /// The current thread holding ticket `seq` for `cmd`.
    fn new(seq: u64, cmd: Cmd, cancel: &Cancel) -> Self {
        Self {
            seq,
            cmd,
            cancel: cancel.clone(),
            thread: thread::current().id(),
        }
    }
}

/// One Box's turn and the ops waiting for it, in arrival order.
#[derive(Debug, Default)]
struct Owner {
    /// Whoever has the turn.
    holder: Option<Holder>,
    /// Tickets waiting for the turn.
    queue: VecDeque<u64>,
    /// Runs whose exec continues after their turn ended.
    runs: usize,
}

impl Owner {
    /// Nothing in flight or waiting.
    fn idle(&self) -> bool {
        self.holder.is_none() && self.queue.is_empty() && self.runs == 0
    }
}

/// Every owner, behind the one lock.
#[derive(Debug, Default)]
struct State {
    /// Owners of the Boxes with something in flight.
    boxes: HashMap<BoxId, Owner>,
    /// The next ticket.
    next: u64,
    /// Set once the controller stops admitting.
    draining: bool,
}

/// The error every waiter and later call gets after a drain.
fn restarting() -> CommandError {
    CommandError::new(ErrorKind::Refused, "controller restarting")
}

impl State {
    /// Admits `cmd` to `box_id`'s queue and returns its ticket, unless it is refused outright.
    fn enter(&mut self, box_id: BoxId, cmd: Cmd) -> Result<u64, CommandError> {
        if self.draining {
            return Err(restarting());
        }
        let ticket = self.next;
        self.next += 1;
        let owner = self.boxes.entry(box_id).or_default();
        let holder = owner.holder.as_ref();
        if holder.is_some_and(|h| h.thread == thread::current().id()) {
            let why = format!(
                "{} called while this thread holds the Box's turn",
                cmd.name()
            );
            return Err(CommandError::new(ErrorKind::Internal, why));
        }
        if owner.queue.len() >= MAX_QUEUE {
            let why = format!("box busy: {} queued", owner.queue.len());
            return Err(CommandError::new(ErrorKind::Refused, why));
        }
        match admit(cmd, holder.map(|h| h.cmd), owner.runs > 0) {
            Admit::Refuse(why) => return Err(CommandError::new(ErrorKind::Refused, why)),
            Admit::Preempt => owner.queue.push_front(ticket),
            Admit::Now | Admit::Queue => owner.queue.push_back(ticket),
        }
        Ok(ticket)
    }

    /// Grants `ticket` the turn when it heads the queue and the policy lets `cmd` in; whether it
    /// did.
    fn grant(
        &mut self,
        box_id: BoxId,
        ticket: u64,
        cmd: Cmd,
        cancel: &Cancel,
    ) -> Result<bool, CommandError> {
        if self.draining {
            return Err(restarting());
        }
        let Some(owner) = self.boxes.get_mut(&box_id) else {
            return Err(CommandError::new(
                ErrorKind::Internal,
                "queued Box lost its owner",
            ));
        };
        if owner.queue.front() != Some(&ticket) {
            return Ok(false);
        }
        match admit(cmd, owner.holder.as_ref().map(|h| h.cmd), owner.runs > 0) {
            Admit::Now => {
                owner.queue.pop_front();
                owner.holder = Some(Holder::new(ticket, cmd, cancel));
                Ok(true)
            }
            Admit::Refuse(why) => Err(CommandError::new(ErrorKind::Refused, why)),
            Admit::Preempt => {
                owner.holder.iter().for_each(|h| h.cancel.cancel());
                Ok(false)
            }
            Admit::Queue => Ok(false),
        }
    }

    /// The refusal for a waiter that gave up on `box_id`.
    fn busy(&self, box_id: BoxId) -> CommandError {
        let holder = self.boxes.get(&box_id).and_then(|o| o.holder.as_ref());
        let busy = holder.map_or("a run", |h| h.cmd.name());
        CommandError::new(ErrorKind::Refused, format!("box busy: {busy} in progress"))
    }

    /// Takes `ticket` out of `box_id`'s queue.
    fn leave(&mut self, box_id: BoxId, ticket: u64) {
        if let Some(owner) = self.boxes.get_mut(&box_id) {
            owner.queue.retain(|t| *t != ticket);
        }
        self.drop_idle(box_id);
    }

    /// Forgets `box_id`'s owner once nothing is in flight on it.
    fn drop_idle(&mut self, box_id: BoxId) {
        if self.boxes.get(&box_id).is_some_and(Owner::idle) {
            self.boxes.remove(&box_id);
        }
    }
}

/// The owners of every Box with an op in flight.
pub(crate) struct Owners {
    /// Every owner.
    state: Mutex<State>,
    /// Signalled whenever a turn, queue or run changes.
    changed: Condvar,
    /// What waiters measure their deadline on.
    clock: Arc<dyn Clock>,
}

/// One Box's turn, released on drop, also when its op panics.
pub(crate) struct Turn<'a> {
    /// Where the turn is released.
    owners: &'a Owners,
    /// The Box whose turn this is.
    box_id: BoxId,
    /// The ticket the turn was granted to.
    seq: u64,
}

/// A run whose exec goes on after its turn; the Box's owner counts it until this drops.
pub(crate) struct RunHold<'a> {
    /// Where the run is counted.
    owners: &'a Owners,
    /// The Box running it.
    box_id: BoxId,
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

    /// Runs `f` under `box_id`'s turn on this thread, once [`admit`] lets `cmd` in, and returns
    /// what `f` returns.
    ///
    /// Waiters are served in arrival order; a forced close of a long holder cancels it and goes
    /// first.
    ///
    /// # Errors
    /// `Refused` when the policy refuses `cmd`, when [`MAX_QUEUE`] ops already wait, when
    /// `deadline` passes or `cancel` is set while waiting, or on [`Owners::drain`]; `Internal`
    /// when this thread already holds `box_id`'s turn. Otherwise `f`'s own result.
    pub(crate) fn call<'a, T>(
        &'a self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        deadline: Instant,
        f: impl FnOnce(&Turn<'a>) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let turn = self.wait_turn(box_id, cmd, cancel, deadline)?;
        f(&turn)
    }

    /// Runs `f` under `box_id`'s turn only when nothing is in flight or waiting on it; `None`
    /// when the Box is busy, so a background pass skips it.
    pub(crate) fn try_call<'a, T>(
        &'a self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        f: impl FnOnce(&Turn<'a>) -> T,
    ) -> Option<T> {
        let mut st = self.lock();
        if st.draining || st.boxes.contains_key(&box_id) {
            return None;
        }
        let seq = st.next;
        st.next += 1;
        st.boxes.entry(box_id).or_default().holder = Some(Holder::new(seq, cmd, cancel));
        drop(st);
        let turn = Turn {
            owners: self,
            box_id,
            seq,
        };
        Some(f(&turn))
    }

    /// Wakes every waiter with a restarting error and refuses every later call; holders finish.
    pub(crate) fn drain(&self) {
        self.lock().draining = true;
        self.changed.notify_all();
    }

    /// Queues `cmd` on `box_id` and blocks until it gets the turn, is refused or gives up.
    fn wait_turn(
        &self,
        box_id: BoxId,
        cmd: Cmd,
        cancel: &Cancel,
        deadline: Instant,
    ) -> Result<Turn<'_>, CommandError> {
        let mut st = self.lock();
        let seq = st.enter(box_id, cmd)?;
        loop {
            let granted = st.grant(box_id, seq, cmd, cancel);
            let gave_up = cancel.is_cancelled() || self.clock.instant() >= deadline;
            let err = match granted {
                Ok(true) => {
                    return Ok(Turn {
                        owners: self,
                        box_id,
                        seq,
                    })
                }
                Ok(false) if !gave_up => None,
                Ok(false) => Some(st.busy(box_id)),
                Err(e) => Some(e),
            };
            if let Some(err) = err {
                st.leave(box_id, seq);
                drop(st);
                self.changed.notify_all();
                return Err(err);
            }
            st = self
                .changed
                .wait_timeout(st, POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Releases ticket `seq`'s turn on `box_id`, or one of its runs when `seq` is `None`, and
    /// wakes the waiters.
    fn release(&self, box_id: BoxId, seq: Option<u64>) {
        let mut st = self.lock();
        if let Some(owner) = st.boxes.get_mut(&box_id) {
            match seq {
                Some(seq) if owner.holder.as_ref().is_some_and(|h| h.seq == seq) => {
                    owner.holder = None;
                }
                Some(_) => {}
                None => owner.runs = owner.runs.saturating_sub(1),
            }
        }
        st.drop_idle(box_id);
        drop(st);
        self.changed.notify_all();
    }

    /// Owners currently held in memory.
    #[cfg(test)]
    fn owned(&self) -> usize {
        self.lock().boxes.len()
    }

    /// Ops waiting for `box_id`'s turn.
    #[cfg(test)]
    fn waiting(&self, box_id: BoxId) -> usize {
        self.lock().boxes.get(&box_id).map_or(0, |o| o.queue.len())
    }
}

impl<'a> Turn<'a> {
    /// Marks a run in flight on this Box until the returned hold drops, past the end of the turn.
    pub(crate) fn begin_run(&self) -> RunHold<'a> {
        if let Some(owner) = self.owners.lock().boxes.get_mut(&self.box_id) {
            owner.runs += 1;
        }
        RunHold {
            owners: self.owners,
            box_id: self.box_id,
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.owners.release(self.box_id, Some(self.seq));
    }
}

impl Drop for RunHold<'_> {
    fn drop(&mut self) {
        self.owners.release(self.box_id, None);
    }
}

#[cfg(test)]
mod tests;
