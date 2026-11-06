//! What a Box's Proxy tells the machine service about the Box's slot, read per request.

use moor_core::domain::boxes::BoxState;
use moor_core::view::MachineHoldState;
use moor_core::BoxId;

use crate::proxy::front::{Held, Hold};
use crate::proxy::keeper::KEEPERS;
use crate::store::Store;

/// The slot `box_id` holds, or why it holds none.
///
/// A wanted claim waits for a slot. An asleep claim belongs to a Box that sleeps, or is about to:
/// its keeper parks before the machine sleeps. Once the Box runs again and its keeper is not
/// parked, the keeper's next tick wants the slot.
pub(super) fn hold_of(store: &Store, box_id: BoxId) -> Hold {
    let missing = |reason: String| Hold::Missing { box_id, reason };
    let Ok(claim) = store.claimed_machine(box_id) else {
        return missing("the Box's machine could not be read; try again later".to_owned());
    };
    let Some(claim) = claim else {
        if let Ok(Some(name)) = store.removed_machine(box_id) {
            return missing(format!("machine {name} was removed"));
        }
        return missing(
            "this Box has no machine yet; call the machine_claim tool to take a slot on one of your user's machines"
                .to_owned(),
        );
    };
    let name = claim.machine.name;
    let awake = match claim.state {
        MachineHoldState::Held => {
            let runner = store.machine_runner(claim.machine_id, store.now());
            return Hold::Held(Held {
                box_id,
                name,
                machine_id: claim.machine_id,
                slot: claim.slot,
                sync_exclude: claim.sync_exclude,
                runner: runner.ok().flatten(),
            });
        }
        MachineHoldState::Waiting => true,
        MachineHoldState::Asleep => {
            let runs = store.box_row(box_id).is_ok_and(|row| {
                row.and_then(|r| r.boxed)
                    .is_some_and(|b| matches!(b.state, BoxState::Ready | BoxState::Starting))
            });
            runs && !KEEPERS.get(box_id).is_some_and(|k| k.parked())
        }
    };
    if awake {
        missing(format!(
            "machine {name} has no free slot yet; this Box takes one as soon as another Box frees it, so try again later"
        ))
    } else {
        missing(format!(
            "machine {name} is released while this Box sleeps; try again later"
        ))
    }
}
