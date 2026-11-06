//! The user's machine attached to a kept Box: its slot granted while the Box is awake and freed
//! while it sleeps, its forwards opened and reopened when a child died, and the machine let go
//! once the Box closes.

use moor_core::domain::event::{Event, MachineEvent};

use moor_core::BoxHandle;

use moor_client::link::reach::Runner;

use super::link::{keep, ForwardFailed, MachineLink, Via, MAX_PIPES};
use super::Kept;
use crate::store::{HeldMachine, MachineWake};

/// A held machine's id, the runner its link row names, and that runner with its name looked up
/// for dialing.
pub(super) type Reach = (i64, Result<(Runner, Runner), String>);

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
    /// child died. The caller holds the keeper's busy lock; the forwards open outside the state
    /// lock and are kept only if the Box is still awake on the same machine.
    pub(super) fn tend_machine(&self, reach: Option<Reach>) {
        self.grant_machine();
        let store = &self.deps.store;
        let want = match store.box_machine(self.box_id) {
            Ok(Some(want)) => want,
            Ok(None) => {
                let gone = std::mem::take(&mut self.lock().machine);
                drop(gone);
                return;
            }
            Err(e) => return self.say(format_args!("cannot read the machine: {e}")),
        };
        let mut inner = self.lock();
        let Some(handle) = inner.handle.clone().filter(|h| self.current(&inner, h)) else {
            return;
        };
        let mut slot = std::mem::take(&mut inner.machine);
        drop(inner);
        let event = self.open_forwards(&mut slot, &handle, want, reach);
        let mut inner = self.lock();
        if !self.current(&inner, &handle) {
            drop(inner);
            drop(slot);
            return;
        }
        inner.machine = slot;
        drop(inner);
        if let Some(event) = event {
            self.record(&Event::Machine(event));
        }
    }

    /// Opens `slot`'s forwards into `handle` for `want` unless they are live; what to record.
    fn open_forwards(
        &self,
        slot: &mut MachineSlot,
        handle: &BoxHandle,
        want: HeldMachine,
        reach: Option<Reach>,
    ) -> Option<MachineEvent> {
        let (name, ports, enrolled) = (want.machine.name, want.ports, want.machine_id);
        let wanted = Some((name.clone(), ports.clone()));
        if slot.opened != wanted {
            slot.link = None;
        }
        // Nothing is looked up under the busy lock: a machine the tick could not read before it
        // opens on the next tick.
        let (_, runner) = reach.filter(|(id, _)| *id == enrolled)?;
        let now = runner.as_ref().ok().map(|(named, _)| named);
        let fresh = !slot.link.as_ref().is_some_and(|l| l.holds(now));
        let opened = keep(&mut slot.link, now, || {
            let (named, dialed) = runner.as_ref().map_err(|error| ForwardFailed {
                port: None,
                error: error.clone(),
            })?;
            let via = Via {
                named,
                dialed,
                box_id: self.box_id.get(),
                machine_id: enrolled,
            };
            MachineLink::open(self.deps.driver.as_ref(), handle, &via, &ports)
        });
        match opened {
            Ok(()) if fresh => {
                slot.opened = wanted;
                slot.failing = false;
                Some(MachineEvent::MachineAttached { name, ports })
            }
            Ok(()) => {
                let over = slot.link.as_ref().and_then(MachineLink::over_cap);
                over.map(|port| MachineEvent::MachineForwardFailed {
                    port: Some(port),
                    error: format!("over {MAX_PIPES} connections at once; one was refused"),
                })
            }
            Err(_) if slot.failing => None,
            Err(ForwardFailed { port, error }) => {
                slot.failing = true;
                Some(MachineEvent::MachineForwardFailed { port, error })
            }
        }
    }

    /// The runner holding the link of the machine the Box holds, read before the busy lock so a
    /// slow name lookup holds up no `end` or park; none when the Box holds no machine.
    pub(super) fn reach_machine(&self) -> Option<Reach> {
        let id = self.deps.store.box_machine(self.box_id).ok()??.machine_id;
        let named = self.deps.runners.runner(id);
        Some((id, named.and_then(|n| Ok((n.clone(), n.looked_up()?)))))
    }

    /// Ends the forwards and frees the slot for another Box, keeping the claim on the machine.
    pub(super) fn sleep_machine(&self) {
        let gone = std::mem::take(&mut self.lock().machine);
        drop(gone);
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
    pub(super) fn let_go_machine(&self) {
        let gone = std::mem::take(&mut self.lock().machine);
        drop(gone);
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
