//! Making room on a full node, by suspending its longest-idle Boxes.

use std::time::Duration;

use moor_core::decide::preempt::{make_room, suspendable, Resident, Room};
use moor_core::decide::schedule::{BoxSpec, Node, Verdict};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::name::BoxId;
use moor_core::seam::driver::SleepOutcome;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::{sleep_handle, sleep_parked};
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::node::{recorded, verdict};
use crate::store::{BoxRow, Store};

/// Suspends Boxes on `node` until one needing `need_mib` fits, when its Box count
/// has room. False when it still does not fit; then nothing more is suspended.
pub(crate) fn make_room_on(
    cx: &Cx,
    store: &Store,
    node: &Node,
    need_mib: u64,
    for_box: &str,
) -> Result<bool, CommandError> {
    let Some(m) = node.memory else {
        return Ok(false);
    };
    if node.max_boxes.is_some_and(|max| node.used >= max) {
        return Ok(false);
    }
    let residents = residents(store, node)?;
    // An over-committed node must first win back what it holds beyond its offer.
    let over_mib = m.used_mib.saturating_sub(m.offered_mib());
    match make_room(m.free_mib(), need_mib.saturating_add(over_mib), &residents) {
        Room::Fits => Ok(true),
        Room::Short => Ok(false),
        Room::Suspend(ids) => {
            for id in ids {
                if !suspend(cx, store, id, &format!("made room for {for_box}"))? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

/// The Boxes up on `node`, as the preemption decider sees them.
fn residents(store: &Store, node: &Node) -> Result<Vec<Resident>, CommandError> {
    let mut out = Vec::new();
    for (id, mem_mib) in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            out.push(resident(store, &row, mem_mib)?);
        }
    }
    Ok(out)
}

/// A Box on a node as the suspend choice sees it.
fn resident(store: &Store, row: &BoxRow, mem_mib: u64) -> Result<Resident, CommandError> {
    let needs_input = store
        .last_waiting_marker(row.id)?
        .is_some_and(|e| e.kind == "needs_input");
    Ok(Resident {
        id: row.id,
        lifecycle: row.lifecycle,
        session: row.session,
        needs_input,
        idle_since: row.idle_since,
        mem_mib,
    })
}

/// Suspends Box `id` for `reason` if it is still idle and its lock is free; false otherwise.
fn suspend(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Ok(_held) = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", Duration::ZERO)
    else {
        return Ok(false);
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    if row.lifecycle != BoxLifecycle::Ready || !suspendable(&resident(store, &row, 0)?) {
        return Ok(false);
    }
    let Ok(handle) = sleep_handle(cx, &row) else {
        return Ok(false);
    };
    if !matches!(
        sleep_parked(cx, store, id, &handle)?,
        SleepOutcome::Slept | SleepOutcome::Native
    ) {
        return Ok(false);
    }
    store.apply(
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: true,
                reason: Some(reason.to_owned()),
            })
            .box_input(BoxInput::Sleep)
            .set(Fields {
                box_sleep_reason: Field::Clear,
                state_reason: Field::Set(reason.to_owned()),
                ..Fields::default()
            }),
    )?;
    Ok(true)
}

/// Before an asleep Box resumes: makes room for it there, or
/// refuses with the node's shortfall.
pub(crate) fn room_to_wake(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let Some(node) = recorded(cx, store, row)? else {
        return Ok(());
    };
    let Some(m) = node.memory else {
        return Ok(());
    };
    let spec = store.box_spec(row.id)?;
    if m.free_mib() >= spec.mem_mib
        || make_room_on(cx, store, &node, spec.mem_mib, row.name.as_str())?
    {
        return Ok(());
    }
    let on_it = BoxSpec {
        node: Some(node.name.clone()),
        ..spec
    };
    let reason = match verdict(cx, (&[node], None), row.user, &on_it) {
        Verdict::Pending { reason, .. } | Verdict::Fail { reason } => reason,
        Verdict::Bind { node, .. } => format!("{node} has no idle Box to suspend for this one"),
    };
    Err(CommandError::new(ErrorKind::Refused, reason)
        .with_hint("close or finish a Box on that node, then wake this one"))
}
