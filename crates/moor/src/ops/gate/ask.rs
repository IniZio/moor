//! The question to the owner: a gate that asks waits for the agent to go quiet, then tells the
//! owner once.
//!
//! A gate that follows a report or a check has a line that moved it, which wrote the event
//! already. One that asks first has no line, so the pass notes it here; the store writes nothing
//! for a wait that has its event.

use moor_core::gate::{status, GateLine, GateNext, GateStatus, Gates};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::quiet::settled;
use crate::store::{BoxRow, Store};

/// Tells the owner of `row` that a gate asks for their answer, when one does and the agent is
/// quiet. Nothing else is done.
///
/// # Errors
/// A store failure.
pub(super) fn ask(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<(), CommandError> {
    let GateStatus::Open {
        next: GateNext::Ask,
        ..
    } = status(gates, lines)
    else {
        return Ok(());
    };
    if !settled(cx, store, row)? {
        return Ok(());
    }
    Ok(store.note_gate_wait(row.id, cx.clock().now())?)
}
