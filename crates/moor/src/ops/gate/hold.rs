//! What a Box with gates that can still move holds back: its close, and what a person's steer
//! does to a stuck one.

use moor_core::domain::event::Origin;
use moor_core::gate::{status, GateLine, GateStatus};
use moor_core::{BoxId, Warning};
use moor_schema::tables::BoxGateLines;

use crate::cx::Cx;
use crate::store::{RunOfBox, Store, StoreError};

/// Whether the gates of Box `id` can still move, so nobody may close it as unneeded: one is open
/// or stuck, and the Box's run, if it has one, has not ended. A Box whose gates failed for good or
/// whose run ended falls back to the ordinary close rules.
///
/// # Errors
/// A store failure.
pub(crate) fn holds(store: &Store, id: BoxId) -> Result<bool, StoreError> {
    let Some(gates) = store.box_gates(id)? else {
        return Ok(false);
    };
    if matches!(store.run_of_box(id)?, RunOfBox::Ended(_)) {
        return Ok(false);
    }
    let now = status(&gates, &store.gate_lines(id)?);
    Ok(matches!(now, GateStatus::Open { .. } | GateStatus::Stuck { .. }))
}

/// Writes `Reopened` for the gate of Box `id` that is stuck, after a person's steer reached its
/// agent: they have taken it up, so the gate gets fresh tries. moor's own steers, a gate that is
/// not stuck, and the Box of a run that ended, whose gates nothing runs, are left alone. A failure
/// is a warning on the steer, which was sent.
pub(crate) fn after_steer(cx: &Cx, store: &Store, id: BoxId, origin: Origin) {
    if origin != Origin::Person {
        return;
    }
    if let Err(e) = reopen(cx, store, id) {
        cx.warn(Warning {
            code: "gate_not_reopened".to_owned(),
            message: format!("the steer was sent, but its gate stays stuck: {e}"),
        });
    }
}

/// Writes `Reopened` when the gate of Box `id` is stuck; one retry when another line took the
/// number, since that line may be the reopening itself.
fn reopen(cx: &Cx, store: &Store, id: BoxId) -> Result<(), StoreError> {
    let Some(gates) = store.box_gates(id)? else {
        return Ok(());
    };
    if matches!(store.run_of_box(id)?, RunOfBox::Ended(_)) {
        return Ok(());
    }
    let by = cx.requester().map_or_else(
        |_| "a person".to_owned(),
        |(who, _)| who.as_str().to_owned(),
    );
    for _ in 0..2 {
        let lines = store.gate_lines(id)?;
        let GateStatus::Stuck { gate, .. } = status(&gates, &lines) else {
            return Ok(());
        };
        let n = i64::try_from(lines.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1);
        let line = GateLine::Reopened {
            gate,
            by: by.clone(),
        };
        match store.append_gate_line(id, n, cx.clock().now(), &line) {
            Err(StoreError::Db(db))
                if db.unique_key().is_some_and(|k| {
                    k.is(&BoxGateLines::Table, &[BoxGateLines::Box, BoxGateLines::N])
                }) => {}
            done => return done,
        }
    }
    Ok(())
}
