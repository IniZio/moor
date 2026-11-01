//! Making room on a full node, by suspending its longest-idle Boxes.

use std::time::Duration;

use moor_core::decide::preempt::{longest_idle, suspendable, Resident};
use moor_core::decide::schedule::{Node, Shortfall};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::name::BoxId;
use moor_core::seam::driver::SleepOutcome;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::{sleep_handle, sleep_parked};
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::node::{now, recorded};
use crate::store::{BoxRow, Store};

/// Suspends the longest-idle Box on `node` when it is short of memory, so that a Box waiting
/// for it starts once the node reports the memory freed. Nothing when its report is stale, its
/// Box count is full, or no Box there is idle.
pub(crate) fn make_room_on(
    cx: &Cx,
    store: &Store,
    node: &Node,
    for_box: &str,
) -> Result<(), CommandError> {
    let short = node.memory.and_then(|m| m.shortfall(now(cx)));
    if !matches!(short, Some(Shortfall::Low { .. } | Shortfall::Pressure))
        || node.max_boxes.is_some_and(|max| node.used >= max)
    {
        return Ok(());
    }
    let mut residents = Vec::new();
    for id in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            residents.push(resident(store, &row)?);
        }
    }
    if let Some(id) = longest_idle(&residents) {
        suspend(cx, store, id, &format!("made room for {for_box}"))?;
    }
    Ok(())
}

/// A Box on a node as the suspend choice sees it.
fn resident(store: &Store, row: &BoxRow) -> Result<Resident, CommandError> {
    let needs_input = store
        .last_waiting_marker(row.id)?
        .is_some_and(|e| e.kind == "needs_input");
    Ok(Resident {
        id: row.id,
        lifecycle: row.lifecycle,
        session: row.session,
        needs_input,
        idle_since: row.idle_since,
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
    if row.lifecycle != BoxLifecycle::Ready || !suspendable(&resident(store, &row)?) {
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

/// Before an asleep Box resumes: why its node has no room for it, after suspending an idle Box
/// there to make some; `None` when it may wake now.
pub(crate) fn room_to_wake(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<Option<String>, CommandError> {
    let Some(node) = recorded(cx, store, row)? else {
        return Ok(None);
    };
    let Some(short) = node.memory.and_then(|m| m.shortfall(now(cx))) else {
        return Ok(None);
    };
    make_room_on(cx, store, &node, row.name.as_str())?;
    Ok(Some(short.reason(&node.name)))
}
