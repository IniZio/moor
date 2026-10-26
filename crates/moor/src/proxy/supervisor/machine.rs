//! The user's machine attached to the supervised Box: opened and revived on each tick, let go
//! when the Box sleeps or closes. A supervisor that stops on a signal keeps the record, so the
//! next supervisor opens the link again.

use moor_core::domain::event::{Event, MachineEvent};

use super::run::{say, Running};
use crate::ops::common::machine::{keep, ForwardFailed, MachineLink};

/// The link to the attached machine and whether its last open failed.
#[derive(Default)]
pub(super) struct MachineSlot {
    /// The running forwards; `None` until opened.
    link: Option<MachineLink>,
    /// The last open failed and was reported; set back when an open succeeds.
    failing: bool,
}

impl Running<'_> {
    /// Opens the recorded machine's forwards, or opens them again when a child died. A sleeping
    /// Box lets its machine go.
    pub(super) fn tend_machine(&self) {
        if self.parked.get() {
            self.let_go_machine();
            return;
        }
        let want = match self.deps.store.box_machine(self.box_id) {
            Ok(Some(want)) => want,
            Ok(None) => return,
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot read the machine: {err}"),
                );
                return;
            }
        };
        let (machine, ports) = want;
        let mut slot = self.machine.borrow_mut();
        let fresh = !slot.link.as_mut().is_some_and(MachineLink::alive);
        let open = || {
            MachineLink::open(
                self.deps.driver,
                self.deps.proc,
                &self.handle,
                &machine,
                &ports,
            )
        };
        let event = match keep(&mut slot.link, open) {
            Ok(()) if fresh => {
                slot.failing = false;
                MachineEvent::MachineAttached {
                    name: machine.name.clone(),
                    ports: ports.clone(),
                }
            }
            Ok(()) => return,
            Err(_) if slot.failing => return,
            Err(ForwardFailed { port, error }) => {
                slot.failing = true;
                MachineEvent::MachineForwardFailed { port, error }
            }
        };
        drop(slot);
        self.record_machine(event);
    }

    /// Ends the forwards and frees the machine for the next Box.
    pub(super) fn let_go_machine(&self) {
        *self.machine.borrow_mut() = MachineSlot::default();
        let name = match self.deps.store.box_machine(self.box_id) {
            Ok(held) => held.map(|(m, _)| m.name),
            Err(_) => None,
        };
        match self.deps.store.detach_machine(self.box_id) {
            Ok(true) => {}
            Ok(false) => return,
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot free the machine: {err}"),
                );
                return;
            }
        }
        if let Some(name) = name {
            self.record_machine(MachineEvent::MachineDetached { name });
        }
    }

    /// Appends `event` to the Box's events while this supervisor holds the lease.
    fn record_machine(&self, event: MachineEvent) {
        let event = Event::Machine(event);
        if let Err(err) = self
            .deps
            .store
            .fenced_event(self.box_id, &self.owner, &event)
        {
            say(
                &self.deps.stderr,
                format_args!("cannot record {}: {err}", event.kind()),
            );
        }
    }
}
