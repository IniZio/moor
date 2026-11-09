//! The owners' shared state: who holds each Box's turn and who waits for it.

use std::collections::{HashMap, VecDeque};
use std::thread::{self, ThreadId};

use moor_core::name::BoxId;
use moor_core::ErrorKind;

use super::policy::{admit, Admit, Cmd};
use crate::env::Cancel;
use crate::error::CommandError;

/// What a waiter heading its queue may do next.
pub(super) enum Grant {
    /// It holds the turn.
    Granted,
    /// It keeps waiting.
    Wait,
    /// It cancelled this long holder and waits for it to let go.
    Preempting(Cmd),
}

/// Most ops that may wait for one Box at once; past it a call is refused.
pub(super) const MAX_QUEUE: usize = 16;

/// Whoever has a Box's turn.
#[derive(Debug)]
pub(super) struct Holder {
    /// The ticket the turn was granted to.
    pub(super) seq: u64,
    /// What the holder is doing.
    pub(super) cmd: Cmd,
    /// The holder's stop flag, which a forced close sets.
    pub(super) cancel: Cancel,
    /// Calling the same Box again from this thread would wait on itself.
    pub(super) thread: ThreadId,
}

impl Holder {
    /// The current thread holding ticket `seq` for `cmd`.
    pub(super) fn new(seq: u64, cmd: Cmd, cancel: &Cancel) -> Self {
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
pub(super) struct Owner {
    /// Whoever has the turn.
    pub(super) holder: Option<Holder>,
    /// Tickets waiting for the turn.
    pub(super) queue: VecDeque<u64>,
    /// Runs whose exec continues after their turn ended.
    pub(super) runs: Vec<ThreadId>,
}

impl Owner {
    /// Nothing in flight or waiting.
    pub(super) fn idle(&self) -> bool {
        self.holder.is_none() && self.queue.is_empty() && self.runs.is_empty()
    }
}

/// Every owner, behind the one lock.
#[derive(Debug, Default)]
pub(super) struct State {
    /// Owners of the Boxes with something in flight.
    pub(super) boxes: HashMap<BoxId, Owner>,
    /// The next ticket.
    pub(super) next: u64,
    /// Set once the controller stops admitting.
    pub(super) draining: bool,
}

/// The error every waiter and later call gets after a drain.
fn restarting() -> CommandError {
    CommandError::new(ErrorKind::Refused, "controller restarting")
}

impl State {
    /// The Box whose turn the current thread holds, if any.
    pub(super) fn held_here(&self) -> Option<BoxId> {
        let me = thread::current().id();
        self.boxes
            .iter()
            .find(|(_, o)| o.holder.as_ref().is_some_and(|h| h.thread == me))
            .map(|(id, _)| *id)
    }

    /// Admits `cmd` to `box_id`'s queue and returns its ticket and how it was admitted, unless it
    /// is refused outright.
    ///
    /// A thread holding a turn may not wait for another: two Boxes each waiting on the other would
    /// deadlock, so crossing to another Box from inside a turn is [`super::Owners::try_turn`]'s job.
    pub(super) fn enter(&mut self, box_id: BoxId, cmd: Cmd) -> Result<(u64, Admit), CommandError> {
        if self.draining {
            return Err(restarting());
        }
        if let Some(held) = self.held_here() {
            let why = format!(
                "{} on Box {} called while this thread holds Box {}'s turn",
                cmd.name(),
                box_id.get(),
                held.get()
            );
            return Err(CommandError::new(ErrorKind::Internal, why));
        }
        let ticket = self.next;
        self.next += 1;
        let owner = self.boxes.entry(box_id).or_default();
        let holder = owner.holder.as_ref();
        if owner.queue.len() >= MAX_QUEUE {
            let why = format!("box busy: {} queued", owner.queue.len());
            return Err(CommandError::new(ErrorKind::Refused, why));
        }
        let admitted = admit(cmd, holder.map(|h| h.cmd), !owner.runs.is_empty());
        match admitted {
            Admit::Refuse(why) => {
                self.drop_idle(box_id);
                return Err(CommandError::new(ErrorKind::Refused, why));
            }
            Admit::Busy => {
                let busy = self.busy(box_id);
                self.drop_idle(box_id);
                return Err(busy);
            }
            Admit::Preempt => owner.queue.push_front(ticket),
            Admit::Now | Admit::Queue => owner.queue.push_back(ticket),
        }
        Ok((ticket, admitted))
    }

    /// Grants `ticket` the turn when it heads the queue and the policy lets `cmd` in.
    pub(super) fn grant(
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
        match admit(
            cmd,
            owner.holder.as_ref().map(|h| h.cmd),
            !owner.runs.is_empty(),
        ) {
            Admit::Now => {
                owner.queue.pop_front();
                owner.holder = Some(Holder::new(ticket, cmd, cancel));
                Ok(Grant::Granted)
            }
            Admit::Refuse(why) => Err(CommandError::new(ErrorKind::Refused, why)),
            Admit::Busy => Err(self.busy(box_id)),
            Admit::Preempt => Ok(owner.holder.as_ref().map_or(Grant::Wait, |h| {
                h.cancel.cancel();
                Grant::Preempting(h.cmd)
            })),
            Admit::Queue => Ok(Grant::Wait),
        }
    }

    /// The refusal for a waiter that gave up on `box_id`.
    pub(super) fn busy(&self, box_id: BoxId) -> CommandError {
        let holder = self.boxes.get(&box_id).and_then(|o| o.holder.as_ref());
        let busy = holder.map_or("a run", |h| h.cmd.name());
        CommandError::new(ErrorKind::Refused, format!("box busy: {busy} in progress"))
    }

    /// Takes `ticket` out of `box_id`'s queue.
    pub(super) fn leave(&mut self, box_id: BoxId, ticket: u64) {
        if let Some(owner) = self.boxes.get_mut(&box_id) {
            owner.queue.retain(|t| *t != ticket);
        }
        self.drop_idle(box_id);
    }

    /// Forgets `box_id`'s owner once nothing is in flight on it.
    pub(super) fn drop_idle(&mut self, box_id: BoxId) {
        if self.boxes.get(&box_id).is_some_and(Owner::idle) {
            self.boxes.remove(&box_id);
        }
    }
}
