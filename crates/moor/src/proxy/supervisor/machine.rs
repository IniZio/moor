//! The user's machine attached to the supervised Box: opened and revived on each tick, its slot
//! freed while the Box sleeps and granted again when it wakes, let go when the Box closes. A
//! supervisor that stops on a signal keeps the record, so the next supervisor opens the link again.

pub(crate) mod link;

use moor_core::domain::event::MachineEvent;

use super::run::{say, Running};
use crate::store::MachineWake;
use link::{keep, ForwardFailed, MachineLink, Via};

/// The link to the attached machine and whether its last open failed.
#[derive(Default)]
pub(super) struct MachineSlot {
    /// The running forwards; `None` until opened.
    link: Option<MachineLink>,
    /// The last open failed and was reported; set back when an open succeeds.
    failing: bool,
    /// The machine and ports the link was opened for.
    opened: Option<(String, Vec<u16>)>,
}

impl Running<'_> {
    /// Opens the recorded machine's forwards, or opens them again when a child died. A sleeping
    /// Box frees its slot; an awake one's want is granted once a slot fits it.
    pub(super) fn tend_machine(&self) {
        if self.parked.get() {
            self.sleep_machine();
            return;
        }
        self.grant_machine();
        let want = match self.deps.ledger.box_machine() {
            Ok(Some(want)) => want,
            Ok(None) => {
                self.machine.borrow_mut().link = None;
                return;
            }
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot read the machine: {err}"),
                );
                return;
            }
        };
        let (name, ports, enrolled) = (want.name, want.ports, want.machine_id);
        let mut slot = self.machine.borrow_mut();
        let wanted = Some((name.clone(), ports.clone()));
        if slot.opened != wanted {
            slot.link = None;
        }
        let fresh = !slot.link.as_mut().is_some_and(MachineLink::alive);
        let event = match keep(&mut slot.link, || self.forwards(&ports, enrolled)) {
            Ok(()) if fresh => {
                // Opening can take long enough for the Box to release the claim meanwhile.
                let now = self.deps.ledger.box_machine();
                let still = now.ok().flatten().map(|h| (h.name, h.ports));
                if still != wanted {
                    slot.link = None;
                    return;
                }
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
        drop(slot);
        self.record_machine(&event);
    }

    /// Opens forwards of `ports` to the machine enrolled under `machine_id`, through the
    /// machine runner.
    fn forwards(&self, ports: &[u16], machine_id: i64) -> Result<MachineLink, ForwardFailed> {
        let socket = crate::runner_dir::of(self.deps.state_dir).join(moor_client::forward::SOCKET);
        let via = Via {
            socket: &socket,
            box_id: self.box_id.get(),
            machine_id,
        };
        MachineLink::open(self.deps.driver, &self.handle, &via, ports)
    }

    /// Ends the forwards and frees the slot for another Box, keeping the claim on the machine.
    fn sleep_machine(&self) {
        *self.machine.borrow_mut() = MachineSlot::default();
        match self.deps.ledger.sleep_machine() {
            Ok(Some(name)) => {
                self.record_machine(&MachineEvent::MachineDetached { name, asleep: true });
            }
            Ok(None) => {}
            Err(err) => say(
                &self.deps.stderr,
                format_args!("cannot free the machine's slot: {err}"),
            ),
        }
    }

    /// Grants the Box's want, an asleep claim wanted again first, saying so when no
    /// slot fits and its place in line changed.
    fn grant_machine(&self) {
        let event = match self.deps.ledger.grant_machine() {
            Ok(MachineWake::Nothing | MachineWake::Woke) => return,
            Ok(MachineWake::Waiting {
                name,
                position,
                reason,
            }) => {
                if self.deps.ledger.waiting_said(position).unwrap_or(false) {
                    return;
                }
                MachineEvent::MachineWaiting {
                    name,
                    position,
                    reason,
                }
            }
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot grant the machine: {err}"),
                );
                return;
            }
        };
        self.record_machine(&event);
    }

    /// Ends the forwards and frees the machine for the next Box.
    pub(super) fn let_go_machine(&self) {
        *self.machine.borrow_mut() = MachineSlot::default();
        let held = self.deps.ledger.box_machine();
        let name = held.ok().flatten().map(|h| h.name);
        match self.deps.ledger.detach_machine() {
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
            self.record_machine(&MachineEvent::MachineDetached {
                name,
                asleep: false,
            });
        }
    }

    /// Appends `event` to the Box's events while this supervisor holds the lease.
    fn record_machine(&self, event: &MachineEvent) {
        if let Err(err) = self.deps.ledger.machine_event(&self.owner, event) {
            say(
                &self.deps.stderr,
                format_args!("cannot record {}: {err}", event.kind()),
            );
        }
    }
}
