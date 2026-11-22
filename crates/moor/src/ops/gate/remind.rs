//! The reminder: a gate that waits for the agent's evidence while the agent is quiet.
//!
//! The first quiet of a try sends the agent one reminder and writes `Reminded`; quiet again in
//! the same try writes `Quiet`, which `status()` reads as stuck. A reminder is sent before its
//! line is written, so one that was not sent is tried again by the next pass.

use moor_core::domain::event::Origin;
use moor_core::gate::{status, Gate, GateLine, GateNext, GateStatus, Gates};
use moor_core::request::BoxTarget;
use moor_core::SteerReq;

use super::checks::taken;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::quiet::settled;
use crate::ops::steer;
use crate::store::{BoxRow, Store};

/// Reminds the agent of `row` of the gate it has not reported, or writes that it went quiet
/// again, when a gate waits for its evidence and the agent is quiet. Nothing else is done.
///
/// # Errors
/// A store failure, or the steer being refused.
pub(super) fn remind(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<(), CommandError> {
    let GateStatus::Open {
        gate,
        try_n,
        next: GateNext::Agent,
    } = status(gates, lines)
    else {
        return Ok(());
    };
    let Some(waiting) = gates.iter().find(|g| g.name == gate) else {
        return Ok(());
    };
    if !settled(cx, store, row)? {
        return Ok(());
    }
    // Try numbers start again after a reopening, so only reminders since the last one count.
    let reminded = lines
        .iter()
        .rev()
        .take_while(|l| !matches!(l, GateLine::Reopened { gate: g, .. } if g == &gate))
        .any(
            |l| matches!(l, GateLine::Reminded { gate: g, try_n: t } if g == &gate && *t == try_n),
        );
    let line = if reminded {
        GateLine::Quiet { gate, try_n }
    } else {
        send(cx, row, waiting)?;
        GateLine::Reminded { gate, try_n }
    };
    let n = i64::try_from(lines.len())
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    match store.append_gate_line(row.id, n, cx.clock().now(), &line) {
        Err(e) if taken(&e) => Ok(()),
        done => Ok(done?),
    }
}

/// Steers the agent of `row` with the reminder for `gate`, as its owner does.
fn send(cx: &Cx, row: &BoxRow, gate: &Gate) -> Result<(), CommandError> {
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(gate.remind_steer()),
        menu: None,
        interrupt: false,
        ask: None,
    };
    cx.for_box(row.user, || steer::run_as(cx, &req, Origin::Moor))
        .map(drop)
}
