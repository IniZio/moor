//! The user's machine attached to a kept Box: its slot granted while the Box is awake and freed
//! while it sleeps, its forwards opened and reopened when a child died, and the machine let go
//! once the Box closes.

use moor_core::domain::event::{Event, MachineEvent};

use super::{Inner, Kept};
use crate::proxy::supervisor::machine_link::{keep, ForwardFailed, MachineLink, Via};
use crate::store::MachineWake;

/// The forwards to the attached machine and whether their last open failed.
#[derive(Default)]
pub(super) struct MachineSlot {
    /// The running forwards; `None` until opened.
    link: Option<MachineLink>,
    /// The last open failed and was reported; set back when an open succeeds.
    failing: bool,
    /// The machine and ports the forwards were opened for.
    opened: Option<(String, Vec<u16>)>,
}

impl Kept {
    /// Grants the Box's want, then opens the held machine's forwards, or opens them again when a
    /// child died.
    pub(super) fn tend_machine(&self, inner: &mut Inner) {
        self.grant_machine();
        let store = &self.deps.store;
        let want = match store.box_machine(self.box_id) {
            Ok(Some(want)) => want,
            Ok(None) => {
                inner.machine = MachineSlot::default();
                return;
            }
            Err(e) => return self.say(format_args!("cannot read the machine: {e}")),
        };
        let Some(handle) = inner.handle.clone() else {
            return;
        };
        let (name, ports, enrolled) = (want.machine.name, want.ports, want.machine_id);
        let slot = &mut inner.machine;
        let wanted = Some((name.clone(), ports.clone()));
        if slot.opened != wanted {
            slot.link = None;
        }
        let fresh = !slot.link.as_mut().is_some_and(MachineLink::alive);
        let via = Via {
            socket: &self.deps.runner,
            box_id: self.box_id.get(),
            machine_id: enrolled,
        };
        let opened = keep(&mut slot.link, || {
            MachineLink::open(self.deps.driver.as_ref(), &handle, &via, &ports)
        });
        let event = match opened {
            Ok(()) if fresh => {
                slot.opened = wanted;
                slot.failing = false;
                MachineEvent::MachineAttached { name, ports }
            }
            Ok(()) => return,
            Err(_) if slot.failing => return,
            Err(ForwardFailed { port, error }) => {
                slot.failing = true;
                MachineEvent::MachineForwardFailed { port, error }
            }
        };
        self.record(&Event::Machine(event));
    }

    /// Ends the forwards and frees the slot for another Box, keeping the claim on the machine.
    pub(super) fn sleep_machine(&self, inner: &mut Inner) {
        inner.machine = MachineSlot::default();
        match self.deps.store.sleep_machine(self.box_id) {
            Ok(Some(name)) => self.record(&Event::Machine(MachineEvent::MachineDetached {
                name,
                asleep: true,
            })),
            Ok(None) => {}
            Err(e) => self.say(format_args!("cannot free the machine's slot: {e}")),
        }
    }

    /// Grants the Box's want, saying so when no slot fits and its place in line changed.
    fn grant_machine(&self) {
        let store = &self.deps.store;
        let event = match store.grant_machine(self.box_id) {
            Ok(MachineWake::Nothing | MachineWake::Woke) => return,
            Ok(MachineWake::Waiting {
                name,
                position,
                reason,
            }) => {
                if store.waiting_said(self.box_id, position).unwrap_or(false) {
                    return;
                }
                MachineEvent::MachineWaiting {
                    name,
                    position,
                    reason,
                }
            }
            Err(e) => return self.say(format_args!("cannot grant the machine: {e}")),
        };
        self.record(&Event::Machine(event));
    }

    /// Ends the forwards and frees the machine for the next Box.
    pub(super) fn let_go_machine(&self, inner: &mut Inner) {
        inner.machine = MachineSlot::default();
        let store = &self.deps.store;
        let name = store.box_machine(self.box_id).ok().flatten();
        match store.detach_machine(self.box_id) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => return self.say(format_args!("cannot free the machine: {e}")),
        }
        if let Some(held) = name {
            self.record(&Event::Machine(MachineEvent::MachineDetached {
                name: held.machine.name,
                asleep: false,
            }));
        }
    }
}
