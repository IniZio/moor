//! Making room on a full node, by suspending its longest-idle Boxes, and relieving a node under
//! memory pressure.

use moor_core::decide::preempt::{longest_idle, suspendable, Resident};
use moor_core::decide::pressure::{
    relieve, sustained, working, Candidate, Cooldown, Reading, SETTLE_SECS,
};
use moor_core::decide::schedule::{Node, Shortfall};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::cache_save::CacheSaveDenied;
use moor_core::domain::decision::{
    Decision, ExtraEvent, Field, Fields, HandsFields, WantChange, Wanted, WantedKind,
};
use moor_core::domain::hands::HandsInput;
use moor_core::domain::session_state::SessionState;
use moor_core::name::{BoxHandle, BoxId};
use moor_core::seam::driver::{PausePublish, SleepOutcome};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::{ready_handle, settled, sleep_handle, sleep_parked};
use crate::ops::common::cache::save_decision;
use crate::ops::common::node::now;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::common::reconcile;
use crate::ops::common::resume_gate::demand;
use crate::ops::common::stale::unless_stale;
use crate::ops::common::suspend_check;
use crate::ops::common::wanted::parking;
use crate::store::{BoxRow, Store};
use disk::{judge_disk, tell_disk};

mod disk;

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
                residents.push(resident(cx, store, &row)?);
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

/// A Box on a node as the suspend choice sees it. An Idle Box whose agent cannot be read may have
/// started a turn unseen, so it counts as working here without leaving Idle.
fn resident(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Resident, CommandError> {
    let needs_input = store
        .last_waiting_marker(row.id)?
        .is_some_and(|e| e.kind == "needs_input");
    Ok(Resident {
        id: row.id,
        lifecycle: row.lifecycle,
        session: if row.session == SessionState::Idle
            && store.agent_unread(row.id, cx.clock().now())?
        {
            SessionState::Working
        } else {
            row.session
        },
        needs_input,
        idle_since: row.idle_since,
    })
}

/// Suspends Box `id` for `reason` if it is still idle, its turn is free and it wants no close;
/// false otherwise. A failure to suspend it is recorded on the Box.
fn suspend(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Some(_turn) = cx.try_turn(id, Cmd::Suspend) else {
        return Ok(false);
    };
    let wanted = Wanted::Suspend {
        reason: reason.to_owned(),
    };
    Ok(parking(store, id, wanted, || suspend_idle(cx, store, id, reason))?.unwrap_or(false))
}

/// [`suspend`] under the turn and the wanted suspend.
fn suspend_idle(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    if row.lifecycle != BoxLifecycle::Ready || !suspendable(&resident(cx, store, &row)?) {
        return Ok(false);
    }
    let Ok(handle) = sleep_handle(cx, &row) else {
        return Ok(false);
    };
    let settled = settled(cx, store, &row)?;
    let (slept, cache_save) = match sleep_parked(cx, store, &row, &handle, settled) {
        Ok(None) => return Ok(false),
        Ok(Some(parked)) => (Ok(parked.outcome), parked.cache_save),
        Err(e) => (Err(e.error.to_string()), None),
    };
    if let Some(error) = why_not_slept(slept) {
        tell_suspend_failed(cx, store, id, reason, &error)?;
        return Ok(false);
    }
    let slept = store.apply(
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: true,
                reason: Some(reason.to_owned()),
            })
            .want(WantChange::Done(WantedKind::Suspend))
            .cache_save(cache_save)
            .hands_input(row.hands_inc(), HandsInput::Sleep)
            .set(Fields {
                state_reason: Field::Set(reason.to_owned()),
                ..Fields::default()
            })
            .hands(
                row.hands_inc(),
                HandsFields {
                    sleep_reason: Field::Clear,
                    ..HandsFields::default()
                },
            ),
    );
    Ok(unless_stale(cx, slept)?.is_some())
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
        tell_disk(cx, store, &node.name, change)?;
        // Saved now, so a suspension failing below does not repeat the alert on every report.
        store.set_pressure(&node.name, &p)?;
    }
    let reserve = memory.reserve();
    p.high_since = sustained(&p.readings, reserve).then(|| p.high_since.unwrap_or(now));
    let mut boxes = Vec::new();
    for id in store.ready_on(&node.name)? {
        if let Some(row) = store.box_row(id)? {
            boxes.push(Candidate {
                resident: resident(cx, store, &row)?,
                started_at: row.created_at.as_millis() / 1000,
                in_flight: reconcile::run_in_flight(cx, store, id)?.is_some(),
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
    // Saved now, so a suspend below judges this report and not the one before it.
    store.set_pressure(&node.name, &p)?;
    for s in picks {
        let done = if s.last_resort {
            suspend_working(cx, store, s.id, &s.reason)?
        } else {
            suspend(cx, store, s.id, &s.reason)?
        };
        if done {
            p.suspended_at = Some(now);
            if s.last_resort {
                store.owe_suspended(s.id, now, false)?;
            }
            break;
        }
    }
    store.set_pressure(&node.name, &p)?;
    Ok(())
}

/// Suspends running Box `id` for `reason` and leaves it waiting for room, if nothing changed, its
/// turn is free and it wants no close; false otherwise. Its agent pane stays open, since the suspension keeps the
/// agent's process, while a run in flight loses its connection. A failure to suspend it is
/// recorded on the Box.
fn suspend_working(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Some(_turn) = cx.try_turn(id, Cmd::Suspend) else {
        return Ok(false);
    };
    let wanted = Wanted::Suspend {
        reason: reason.to_owned(),
    };
    Ok(parking(store, id, wanted, || suspend_running(cx, store, id, reason))?.unwrap_or(false))
}

/// [`suspend_working`] under the turn and the wanted suspend.
fn suspend_running(cx: &Cx, store: &Store, id: BoxId, reason: &str) -> Result<bool, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    let now = resident(cx, store, &row)?;
    if !working(&now) && !suspendable(&now) {
        return Ok(false);
    }
    let Ok(handle) = ready_handle(&row) else {
        return Ok(false);
    };
    let Some(suspended) = park_and_suspend(cx, store, &row, &handle)? else {
        return Ok(false);
    };
    if let Some(error) = why_not_slept(suspended) {
        restore_tunnel(cx, id)?;
        tell_suspend_failed(cx, store, id, reason, &error)?;
        return Ok(false);
    }
    let slept = store.apply(
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Suspend {
                reason: reason.to_owned(),
            })
            .cache_save(save_decision(
                cx,
                store,
                &row,
                Err(CacheSaveDenied::LastResort),
            ))
            .hands_input(row.hands_inc(), HandsInput::Sleep)
            .set(Fields {
                state_reason: Field::Set(reason.to_owned()),
                ..Fields::default()
            })
            .hands(
                row.hands_inc(),
                HandsFields {
                    sleep_reason: Field::Clear,
                    ..HandsFields::default()
                },
            ),
    );
    Ok(unless_stale(cx, slept)?.is_some())
}

/// Parks `row`'s tunnel and asks the Driver to suspend the Box, saving nothing; none when its
/// hands were replaced first.
fn park_and_suspend(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    handle: &BoxHandle,
) -> Result<Option<Result<SleepOutcome, String>>, CommandError> {
    let at = (row.id, row.hands_inc());
    if unless_stale(cx, store.set_pause_publish(at, PausePublish::Denied))?.is_none() {
        return Ok(None);
    }
    // Parking takes the Proxy away, and the check runs in the Box.
    suspend_check::record(cx, store, at, settled(cx, store, row)?.is_ok());
    park_tunnel(cx, row.id)?;
    let slept = cx.driver().sleep(handle, PausePublish::Denied);
    Ok(Some(slept.map_err(|e| e.to_string())))
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
