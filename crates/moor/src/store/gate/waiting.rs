//! The Box event a gate line writes when it makes a gate wait for a person.
//!
//! It is written in the transaction of the line, so the line that moved the gate writes it once:
//! of two writers of one line number only the first gets there, and a line that changes nothing
//! writes none.

use moor_core::domain::event::{Event, WorkflowEvent};
use moor_core::gate::{status, wait, GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxGateLines, Boxes};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, OptionalExtension as _, Tx};
use crate::store::StoreError;

/// Writes the `gate_waiting` event of Box `id` when the line just appended moved its gates to a
/// wait for a person.
pub(super) fn note(tx: &Tx<'_>, id: BoxId, at: Ts) -> Result<(), StoreError> {
    let text: Option<Option<String>> = tx
        .row(
            Query::select()
                .column(Boxes::Gates)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
            |r| r.get(0),
        )
        .optional()?;
    let Some(text) = text.flatten() else {
        return Ok(());
    };
    let gates = Gates::from_json(&text).map_err(|e| StoreError::Corrupt(format!("gates: {e}")))?;
    let lines: Vec<GateLine> = tx
        .rows(
            Query::select()
                .column(BoxGateLines::Entry)
                .from(BoxGateLines::Table)
                .and_where(Expr::col(BoxGateLines::Box).eq(id.get()))
                .order_by(BoxGateLines::N, Order::Asc),
            |r| r.get::<_, String>(0),
        )?
        .map(|row| {
            serde_json::from_str(&row?).map_err(|e| StoreError::Corrupt(format!("gate line: {e}")))
        })
        .collect::<Result<_, StoreError>>()?;
    let Some((_, before)) = lines.split_last() else {
        return Ok(());
    };
    if status(&gates, before) == status(&gates, &lines) {
        return Ok(());
    }
    let Some(waits) = wait(&gates, &lines) else {
        return Ok(());
    };
    let event = Event::Workflow(WorkflowEvent::GateWaiting {
        question: waits.question(),
        gate: waits.gate,
        wait: waits.kind.as_str().to_owned(),
        seq: waits.seq,
    });
    insert_event(tx, id, &event, at)?;
    Ok(())
}
