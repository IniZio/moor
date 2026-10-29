//! What the Box's Proxy tells the machine service about the Box's slot, read per request.

use moor_core::view::MachineHoldState;
use moor_core::BoxId;

use crate::proxy::front::{Held, Hold};
use crate::store::{Store, TunnelWant};

/// The slot `box_id` holds, or why it holds none.
///
/// A wanted claim waits for a slot. An asleep claim belongs to a Box that sleeps unless the latest
/// tunnel request wants the tunnel up: then the Box is awake and its next tick wants the slot.
pub(in crate::proxy) fn hold_of(store: &Store, box_id: BoxId) -> Hold {
    let missing = |reason: String| Hold::Missing { box_id, reason };
    let Ok(claim) = store.claimed_machine(box_id) else {
        return missing("the Box's machine could not be read".to_owned());
    };
    let Some(claim) = claim else {
        return missing(
            "this Box has no machine: its repo's .moor.yaml asks for none, or none was free when it started"
                .to_owned(),
        );
    };
    let name = claim.machine.name;
    let awake = match claim.state {
        MachineHoldState::Held => {
            return Hold::Held(Held {
                box_id,
                name,
                ssh: claim.machine.ssh,
                machine_id: claim.machine_id,
                slot: claim.slot,
                sync_exclude: claim.sync_exclude,
            })
        }
        MachineHoldState::Waiting => true,
        MachineHoldState::Asleep => store
            .tunnel_request(box_id)
            .is_ok_and(|r| r.is_some_and(|r| r.want != TunnelWant::Parked)),
    };
    if awake {
        missing(format!(
            "machine {name} has no free slot yet; this Box takes one as soon as another Box frees it"
        ))
    } else {
        missing(format!("machine {name} is released while this Box sleeps"))
    }
}
