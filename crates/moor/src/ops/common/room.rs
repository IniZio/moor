//! Making room on a full node, by suspending its longest-idle Boxes, and relieving a node under
//! memory pressure.

use std::time::Duration;

use moor_core::decide::preempt::{longest_idle, suspendable, Resident};
use moor_core::decide::pressure::{
    relieve, sustained, working, Candidate, Cooldown, Demand, Reading,
};
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
use crate::ops::common::park::{park_tunnel, restore_tunnel};
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

/// On a new memory report from `node`: keeps it, tracks whether pressure is sustained, and
/// suspends at most one Box there to relieve it. A working Box suspended as a last resort waits
/// for room and resumes first.
pub(crate) fn on_report(
    cx: &Cx,
    store: &Store,
    node: &Node,
    reading: Reading,
) -> Result<(), CommandError> {
    let Some(memory) = node.memory else {
        return Ok(());
    };
    let mut p = store.pressure(&node.name)?;
    if p.readings.last().is_some_and(|r| r.at >= reading.at) {
        return Ok(());
    }
    p.readings.push(reading);
    let now = now(cx);
    let reserve = memory.reserve();
    p.high_since = sustained(&p.readings, reserve).then(|| p.high_since.unwrap_or(now));
    let mut boxes = Vec::new();
    for id in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            boxes.push(Candidate {
                resident: resident(store, &row)?,
                started_at: row.created_at.as_millis() / 1000,
            });
        }
    }
    let pick = relieve(
        &node.name,
        &p.readings,
        reserve,
        &boxes,
        Cooldown {
            suspended_at: p.suspended_at,
            now,
        },
        &demand(store, node, memory.shortfall(now))?,
    );
    if let Some(s) = pick {
        let done = if s.last_resort {
            suspend_working(cx, store, s.id, &s.reason)?
        } else {
            suspend(cx, store, s.id, &s.reason)?
        };
        if done {
            p.suspended_at = Some(now);
            if s.last_resort {
                store.set_resume_first(s.id, Some(now))?;
            }
        }
    }
    store.set_pressure(&node.name, &p)?;
    Ok(())
}

/// What the Boxes waiting on `node` ask of it, given its memory `short`fall. Nothing when its Box
/// count is full, since a suspension would not help them.
fn demand(store: &Store, node: &Node, short: Option<Shortfall>) -> Result<Demand, CommandError> {
    let short = matches!(short, Some(Shortfall::Low { .. } | Shortfall::Pressure))
        && node.max_boxes.is_none_or(|max| node.used < max);
    let waiter = match store.pending_on(&node.name)?.first() {
        Some(id) if short => store.box_row(*id)?.map(|row| row.name.as_str().to_owned()),
        _ => None,
    };
    Ok(Demand { short, waiter })
}

/// Suspends working Box `id` for `reason` and leaves it waiting for room, if it still works and
/// its lock is free; false otherwise. Its agent pane stays open, since the suspension keeps the
/// agent's process.
fn suspend_working(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Ok(_held) = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", Duration::ZERO)
    else {
        return Ok(false);
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    if !working(&resident(store, &row)?) {
        return Ok(false);
    }
    let Ok(handle) = sleep_handle(cx, &row) else {
        return Ok(false);
    };
    park_tunnel(cx, store, id)?;
    if !matches!(
        cx.driver().sleep(&handle),
        Ok(SleepOutcome::Slept | SleepOutcome::Native)
    ) {
        restore_tunnel(cx, store, id, true)?;
        return Ok(false);
    }
    store.apply(
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Suspend {
                reason: reason.to_owned(),
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
