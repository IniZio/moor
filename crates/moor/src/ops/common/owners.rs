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
pub(crate) use self::turn::Turn;
use crate::cx::Cancel;
use crate::error::CommandError;

mod policy;
mod turn;

/// Most ops that may wait for one Box at once; past it a call is refused.
const MAX_QUEUE: usize = 16;

/// How long a forced close waits for the start it cancelled to let go.
pub(crate) const INTERRUPT_WAIT: Duration = Duration::from_secs(30);

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

/// What a waiter heading its queue may do next.
enum Grant {
    /// It holds the turn.
    Granted,
    /// It keeps waiting.
    Wait,
    /// It cancelled this long holder and waits for it to let go.
    Preempting(Cmd),
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

/// The refusal for a forced close whose cancelled `holder` kept the turn past [`INTERRUPT_WAIT`].
fn not_stopped(holder: Cmd) -> CommandError {
    let why = format!("box busy: {} did not stop after cancel", holder.name());
    CommandError::new(ErrorKind::Refused, why)
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

    /// Grants `ticket` the turn when it heads the queue and the policy lets `cmd` in.
    fn grant(
        &mut self,
        box_id: BoxId,
        ticket: u64,
        cmd: Cmd,
        cancel: &Cancel,
    ) -> Result<Grant, CommandError> {
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
            return Ok(Grant::Wait);
        }
        match admit(cmd, owner.holder.as_ref().map(|h| h.cmd), owner.runs > 0) {
            Admit::Now => {
                owner.queue.pop_front();
                owner.holder = Some(Holder::new(ticket, cmd, cancel));
                Ok(Grant::Granted)
            }
            Admit::Refuse(why) => Err(CommandError::new(ErrorKind::Refused, why)),
            Admit::Preempt => Ok(owner.holder.as_ref().map_or(Grant::Wait, |h| {
                h.cancel.cancel();
                Grant::Preempting(h.cmd)
            })),
            Admit::Queue => Ok(Grant::Wait),
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
    /// Waiters are served in arrival order. A forced close of a long holder cancels it and goes
    /// first; one the holder still blocks after [`INTERRUPT_WAIT`] is refused, never run beside it.
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
        Some(f(&Turn::new(self, box_id, seq)))
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

#[cfg(test)]
mod tests;
