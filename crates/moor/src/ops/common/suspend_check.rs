//! What a Box holds, recorded just before it is suspended with its reports and artifacts saved: a
//! suspended guest cannot change, so a close can trust the answer and need not find room to wake
//! the Box.
//!
//! An answer stands only while the guest has not run since. Every suspend writes or clears it,
//! a Box found stopped clears it, and the store clears it with any wake or new machine.

use std::time::{Duration, Instant};

use moor_core::decide::pressure::critical;
use moor_core::{BoxId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{probe_within, BoxWork};
use crate::ops::common::node::now;
use crate::ops::common::{artifacts, reports};
use crate::store::{BoxRow, Store, SuspendCheck};

/// Longest a suspend waits to learn what the Box holds.
const CHECK_WITHIN: Duration = Duration::from_secs(10);

/// Records what Box `id` holds, or that nothing is known of it when it is at work, the check
/// cannot answer, or memory is too short to spend time asking. Never stops the suspend that follows.
pub(crate) fn record(cx: &Cx, store: &Store, id: BoxId) {
    let answer = answer(cx, store, id).and_then(|check| Ok(store.set_suspend_check(id, check)?));
    if let Err(e) = answer {
        cx.warn(Warning {
            code: "suspend_check_failed".to_owned(),
            message: format!("cannot record what Box {} holds: {}", id.get(), e.error),
        });
    }
}

/// What the Box says it holds, unless memory is critical on its node.
fn answer(cx: &Cx, store: &Store, id: BoxId) -> Result<Option<SuspendCheck>, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(None);
    };
    let Some(boxed) = row.boxed.as_ref() else {
        return Ok(None);
    };
    // An agent or run at work could write between the check and the snapshot, and a stored clean
    // lets a close delete the Box unwoken.
    let at_work = !row.session.is_quiet() || store.in_flight_run(id)?.is_some();
    if at_work || memory_critical(store, &row)? {
        return Ok(None);
    }
    let began = cx.clock().instant();
    let (ahead, dirty) = match probe_within(cx, &row, boxed, CHECK_WITHIN) {
        BoxWork::Clean | BoxWork::NoCheckout => (0, 0),
        BoxWork::Unsaved { ahead, dirty } => (ahead, dirty),
        BoxWork::Gone | BoxWork::Unknown(_) => return Ok(None),
    };
    // A close that trusts this answer deletes the Box unwoken, so nothing may be left behind in it.
    let collected = reports::collect_within(cx, store, &row, left(cx, began))
        .and_then(|()| artifacts::collect_within(cx, store, &row, left(cx, began)));
    if let Err(reason) = collected {
        cx.warn(Warning {
            code: "suspend_collect_failed".to_owned(),
            message: format!("not everything in Box {} was saved: {reason}", id.get()),
        });
        return Ok(None);
    }
    Ok(Some(SuspendCheck {
        ahead,
        dirty,
        at: now(cx),
    }))
}

/// What is left of [`CHECK_WITHIN`] since `began`.
fn left(cx: &Cx, began: Instant) -> Duration {
    CHECK_WITHIN.saturating_sub(cx.clock().instant().saturating_duration_since(began))
}

/// Whether the latest report of `row`'s node shows memory critical.
fn memory_critical(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let Some(node) = store.box_spec(row.id)?.node else {
        return Ok(false);
    };
    let pressure = store.pressure(&node)?;
    Ok(pressure.readings.last().is_some_and(critical))
}
