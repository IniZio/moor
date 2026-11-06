//! Making room on a full node, by suspending its longest-idle Boxes, and relieving a node under
//! memory pressure.

use std::time::Duration;

use moor_core::decide::disk::{judge, Change, Report, State};
use moor_core::decide::preempt::{longest_idle, suspendable, Resident};
use moor_core::decide::pressure::{
    relieve, sustained, working, Candidate, Cooldown, Reading, SETTLE_SECS,
};
use moor_core::decide::schedule::{Memory, Node, Shortfall};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::name::{BoxId, NodeName};
use moor_core::seam::driver::SleepOutcome;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::{ready_handle, sleep_handle, sleep_parked};
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::node::now;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::common::resume_gate::demand;
use crate::store::{BoxRow, Store};

/// How long the same failure to suspend a Box goes unrepeated in its events, in seconds.
const FAILURE_REPEAT_SECS: i64 = 600;

/// Suspends the longest-idle Box on `node` when it is short of memory, so that a Box waiting
/// for it starts once the node reports the memory freed. Nothing when its report is stale, its
/// Box count is full, or no Box there is idle.
pub(crate) fn make_room_on(
    cx: &Cx,
    store: &Store,
    node: &Node,
    for_box: &str,
) -> Result<(), CommandError> {
    node.memory
        .and_then(|m| m.shortfall(now(cx)))
        .map_or(Ok(()), |short| {
            suspend_idlest(cx, store, node, short, for_box)
        })
}

/// Suspends the longest-idle settled Box on `node` for `for_box` when `short` is a shortage of
/// memory a suspension can help.
pub(super) fn suspend_idlest(
    cx: &Cx,
    store: &Store,
    node: &Node,
    short: Shortfall,
    for_box: &str,
) -> Result<(), CommandError> {
    if !helps(node, short) {
        return Ok(());
    }
    let mut residents = Vec::new();
    for id in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            let settled =
                resumed_at(store, id)?.is_none_or(|at| now(cx).saturating_sub(at) >= SETTLE_SECS);
            if settled {
                residents.push(resident(store, &row)?);
            }
        }
    }
    if let Some(id) = longest_idle(&residents) {
        suspend(cx, store, id, &format!("made room for {for_box}"))?;
    }
    Ok(())
}

/// Whether suspending a Box on `node` can help with `short`: not when its Box count is full.
pub(super) fn helps(node: &Node, short: Shortfall) -> bool {
    matches!(short, Shortfall::Low { .. } | Shortfall::Pressure)
        && node.max_boxes.is_none_or(|max| node.used < max)
}

/// When Box `id` last resumed, unix seconds.
fn resumed_at(store: &Store, id: BoxId) -> Result<Option<i64>, CommandError> {
    Ok(store
        .last_event_of(id, "woke")?
        .map(|ev| ev.ts.as_millis() / 1000))
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

/// Suspends Box `id` for `reason` if it is still idle and its lock is free; false otherwise. A
/// failure to suspend it is recorded on the Box.
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
    let slept = sleep_parked(cx, store, id, &handle).map_err(|e| e.error.to_string());
    if let Some(error) = why_not_slept(slept) {
        tell_suspend_failed(cx, store, id, reason, &error)?;
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
    let now = now(cx);
    let (reading, change) = judge_disk(&memory, p.readings.last(), reading, now);
    p.readings.push(reading);
    if let Some(change) = change {
        tell_disk(store, &node.name, change)?;
        // Saved now, so a suspension failing below does not repeat the alert on every report.
        store.set_pressure(&node.name, &p)?;
    }
    let reserve = memory.reserve();
    p.high_since = sustained(&p.readings, reserve).then(|| p.high_since.unwrap_or(now));
    let mut boxes = Vec::new();
    for id in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            boxes.push(Candidate {
                resident: resident(store, &row)?,
                started_at: row.created_at.as_millis() / 1000,
                in_flight: store.in_flight_run(id)?.is_some(),
                resumed_at: resumed_at(store, id)?,
            });
        }
    }
    let picks = relieve(
        &node.name,
        &p.readings,
        reserve,
        &boxes,
        Cooldown {
            suspended_at: p.suspended_at,
            now,
        },
        &demand(store, node, &memory, now)?,
    );
    for s in picks {
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
            break;
        }
    }
    store.set_pressure(&node.name, &p)?;
    Ok(())
}

/// `reading` marked with whether the node's disk is low after it, and the crossing it made.
fn judge_disk(
    memory: &Memory,
    last: Option<&Reading>,
    reading: Reading,
    now: i64,
) -> (Reading, Option<Change>) {
    let was = if last.is_some_and(|r| r.disk_low) {
        State::Low
    } else {
        State::Ok
    };
    let (disk, change) = judge(
        was,
        Report {
            at: reading.at,
            free_mib: memory.disk_free_mib,
            total_mib: memory.disk_total_mib,
        },
        now,
    );
    let reading = Reading {
        disk_low: disk == State::Low,
        ..reading
    };
    (reading, change)
}

/// Records `change` of `node`'s disk once for each owner with a Box there, so each hears through
/// the webhooks and streams that already carry Box events. Nothing else happens to the Boxes.
fn tell_disk(store: &Store, node: &NodeName, change: Change) -> Result<(), CommandError> {
    let ids = store.one_live_per_owner_on(node)?;
    let node = node.to_string();
    for id in ids {
        let event = match change {
            Change::WentLow {
                free_mib,
                total_mib,
            } => ExtraEvent::NodeDiskLow {
                node: node.clone(),
                free_mib,
                total_mib,
            },
            Change::Recovered {
                free_mib,
                total_mib,
            } => ExtraEvent::NodeDiskRecovered {
                node: node.clone(),
                free_mib,
                total_mib,
            },
        };
        store.apply(Decision::new(id).event(event))?;
    }
    Ok(())
}

/// Suspends running Box `id` for `reason` and leaves it waiting for room, if nothing changed and
/// its lock is free; false otherwise. Its agent pane stays open, since the suspension keeps the
/// agent's process, while a run in flight loses its connection. A failure to suspend it is
/// recorded on the Box.
fn suspend_working(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Ok(_held) = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", Duration::ZERO)
    else {
        return Ok(false);
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    let now = resident(store, &row)?;
    if !working(&now) && !suspendable(&now) {
        return Ok(false);
    }
    let Ok(handle) = ready_handle(&row) else {
        return Ok(false);
    };
    park_tunnel(cx, store, id)?;
    let slept = cx.driver().sleep(&handle).map_err(|e| e.to_string());
    if let Some(error) = why_not_slept(slept) {
        restore_tunnel(cx, store, id, true)?;
        tell_suspend_failed(cx, store, id, reason, &error)?;
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

/// Why a sleep that ended as `slept` left the Box up; `None` when it slept.
fn why_not_slept(slept: Result<SleepOutcome, String>) -> Option<String> {
    match slept {
        Ok(SleepOutcome::Slept | SleepOutcome::Native) => None,
        Ok(SleepOutcome::CannotSleep(why)) => Some(why),
        Err(e) => Some(e),
    }
}

/// Records that suspending Box `id` for `why` failed with `error`. A report retries every 10
/// seconds, so the same failure is recorded again only after [`FAILURE_REPEAT_SECS`].
fn tell_suspend_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    why: &str,
    error: &str,
) -> Result<(), CommandError> {
    let text = format!("could not suspend ({why}): {error}");
    let told = store.last_event_of(id, "stays_up")?.is_some_and(|ev| {
        ev.payload.get("reason").and_then(|r| r.as_str()) == Some(&text)
            && now(cx).saturating_sub(ev.ts.as_millis() / 1000) < FAILURE_REPEAT_SECS
    });
    if !told {
        store.apply(Decision::new(id).event(ExtraEvent::StaysUp { reason: text }))?;
    }
    Ok(())
}
