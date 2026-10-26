//! The user's machine attached to the supervised Box: opened and revived on each tick, its slot
//! freed while the Box sleeps and taken again when it wakes, let go when the Box closes. A
//! supervisor that stops on a signal keeps the record, so the next supervisor opens the link again.

mod hold;
mod link;

use moor_core::domain::event::{Event, MachineEvent};

use super::run::{say, Running};
use crate::store::MachineWake;
pub(super) use hold::hold_of;
use link::{keep, ForwardFailed, MachineLink};

/// The link to the attached machine and whether its last open failed.
#[derive(Default)]
pub(super) struct MachineSlot {
    /// The running forwards; `None` until opened.
    link: Option<MachineLink>,
    /// The last open failed and was reported; set back when an open succeeds.
    failing: bool,
    /// The wake found no free slot and said so; set back when the Box takes one or sleeps.
    waiting: bool,
}

impl Running<'_> {
    /// Opens the recorded machine's forwards, or opens them again when a child died. A sleeping
    /// Box frees its slot; a woken one takes a free slot on the same machine again.
    pub(super) fn tend_machine(&self) {
        if self.parked.get() {
            self.sleep_machine();
            return;
        }
        self.wake_machine();
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
        let (machine, ports) = (want.machine, want.ports);
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

    /// Ends the forwards and frees the slot for another Box, keeping the claim on the machine.
    fn sleep_machine(&self) {
        *self.machine.borrow_mut() = MachineSlot::default();
        match self.deps.store.sleep_machine(self.box_id) {
            Ok(Some(name)) => {
                self.record_machine(MachineEvent::MachineDetached { name, asleep: true });
            }
            Ok(None) => {}
            Err(err) => say(
                &self.deps.stderr,
                format_args!("cannot free the machine's slot: {err}"),
            ),
        }
    }

    /// Takes a free slot on the machine an asleep claim names, saying once per wait when none is.
    fn wake_machine(&self) {
        let event = match self.deps.store.wake_machine(self.box_id) {
            Ok(MachineWake::Nothing) => return,
            Ok(MachineWake::Woke) => {
                self.machine.borrow_mut().waiting = false;
                return;
            }
            Ok(MachineWake::Waiting { name, reason }) => {
                if std::mem::replace(&mut self.machine.borrow_mut().waiting, true) {
                    return;
                }
                MachineEvent::MachineWaiting { name, reason }
            }
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot wake the machine: {err}"),
                );
                return;
            }
        };
        self.record_machine(event);
    }

    /// Ends the forwards and frees the machine for the next Box.
    pub(super) fn let_go_machine(&self) {
        *self.machine.borrow_mut() = MachineSlot::default();
        let held = self.deps.store.box_machine(self.box_id);
        let name = held.ok().flatten().map(|h| h.machine.name);
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
            self.record_machine(MachineEvent::MachineDetached {
                name,
                asleep: false,
            });
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
