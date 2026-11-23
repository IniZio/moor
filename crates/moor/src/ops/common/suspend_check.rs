//! What a Box holds, recorded just before it is suspended with its reports and artifacts saved: a
//! suspended guest cannot change, so a close can trust the answer and need not find room to wake
//! the Box.
//!
//! An answer stands only while the guest has not run since. Every suspend writes or clears it,
//! a Box found stopped clears it, and the store clears it with any wake or new machine.

use std::time::{Duration, Instant};

use moor_core::decide::pressure::critical;
use moor_core::domain::hands::HandsInc;
use moor_core::{BoxId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{probe_within, BoxWork};
use crate::ops::common::node::now;
use crate::ops::common::stale::unless_stale;
use crate::ops::common::{artifacts, reports};
use crate::store::{BoxRow, Store, StoreError, SuspendCheck};

/// Longest a suspend waits to learn what the Box holds.
const CHECK_WITHIN: Duration = Duration::from_secs(10);

/// Records what Box `id` holds, or that nothing is known of it, and why, when it is not `settled`,
/// the check cannot answer, or memory is too short to spend time asking. Failing to learn what it
/// holds never stops the suspend that follows. False when the Box's hands `inc` were replaced, which also
/// stops it: nothing is recorded and the suspend would reach a machine the Box no longer has.
///
/// A row can read quiet only because the sleep marked it so, hence `settled`. Work could write
/// between the check and the snapshot, and a stored clean lets a close delete the Box unwoken.
///
/// # Errors
/// A [`StoreError`] when the answer cannot be written.
pub(crate) fn record(
    cx: &Cx,
    store: &Store,
    (id, inc): (BoxId, HandsInc),
    settled: bool,
) -> Result<bool, StoreError> {
    let answer = if settled {
        answer(cx, store, id).unwrap_or_else(|e| {
            cx.warn(Warning {
                code: "suspend_check_failed".to_owned(),
                message: format!("cannot record what Box {} holds: {}", id.get(), e.error),
            });
            Err(format!("the check failed: {}", e.error))
        })
    } else {
        Err("its agent or a command in it may still have been at work".to_owned())
    };
    let written = match answer {
        Ok(check) => store.set_suspend_check((id, inc), Some(check)),
        Err(why) => store.set_suspend_unchecked((id, inc), &why),
    };
    Ok(unless_stale(cx, written)?.is_some())
}

/// What the Box says it holds, unless memory is critical on its node; else why nothing is known.
fn answer(cx: &Cx, store: &Store, id: BoxId) -> Result<Result<SuspendCheck, String>, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(Err("the Box was gone".to_owned()));
    };
    let Some(boxed) = row.hands.as_ref() else {
        return Ok(Err("it had no machine".to_owned()));
    };
    if memory_critical(store, &row)? {
        return Ok(Err("memory on its node was critical".to_owned()));
    }
    let began = cx.clock().instant();
    let (ahead, dirty) = match probe_within(cx, &row, boxed, CHECK_WITHIN) {
        BoxWork::Clean | BoxWork::NoCheckout => (0, 0),
        BoxWork::Unsaved { ahead, dirty } => (ahead, dirty),
        BoxWork::Gone => return Ok(Err("its machine could not be reached".to_owned())),
        BoxWork::Unknown(why) => return Ok(Err(format!("the check failed: {why}"))),
    };
    // A close that trusts this answer deletes the Box unwoken, so nothing may be left behind in it.
    let collected = reports::collect_within(cx, store, &row, left(cx, began))
        .and_then(|()| artifacts::collect_within(cx, store, &row, left(cx, began)));
    if let Err(reason) = collected {
        cx.warn(Warning {
            code: "suspend_collect_failed".to_owned(),
            message: format!("not everything in Box {} was saved: {reason}", id.get()),
        });
        return Ok(Err(format!("not everything in it was saved: {reason}")));
    }
    Ok(Ok(SuspendCheck {
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
