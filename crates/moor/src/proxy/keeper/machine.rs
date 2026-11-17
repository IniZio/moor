//! The user's machine attached to a kept Box: its slot granted while the Box is awake and freed
//! while it sleeps, and the machine let go once the Box closes.

use moor_core::domain::event::{Event, MachineEvent};

use super::Kept;
use crate::store::{MachineWake, StoreError};

impl Kept {
    /// Frees the slot for another Box, keeping the claim on the machine.
    pub(super) fn sleep_machine(&self) {
        let inc = self.lock().inc;
        match self.deps.store.sleep_machine(self.box_id, inc) {
            Ok(Some(name)) => self.record(&Event::Machine(MachineEvent::MachineDetached {
                name,
                asleep: true,
            })),
            Ok(None) => {}
            Err(e @ StoreError::Stale { .. }) => self.say(format_args!("stale_hands_write: {e}")),
            Err(e) => self.say(format_args!("cannot free the machine's slot: {e}")),
        }
    }

    /// Grants the Box's want, saying so when no slot fits and its place in line changed.
    pub(super) fn grant_machine(&self) {
        let store = &self.deps.store;
        let inc = self.lock().inc;
        let event = match store.grant_machine(self.box_id, inc) {
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
            Err(e @ StoreError::Stale { .. }) => {
                return self.say(format_args!("stale_hands_write: {e}"))
            }
            Err(e) => return self.say(format_args!("cannot grant the machine: {e}")),
        };
        self.record(&Event::Machine(event));
    }

    /// Frees the machine for the next Box.
    pub(super) fn let_go_machine(&self) {
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
