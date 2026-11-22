//! The Box event that makes a gate wait for a person.
//!
//! A gate line writes it in the transaction of the line, so the line that moved the gate writes it
//! once: of two writers of one line number only the first gets there, and a line that changes
//! nothing writes none. A gate that waits before any line exists, such as one that only asks the
//! owner, is noted by [`ensure`], which writes nothing for a wait that has its event already.

use moor_core::domain::event::{Event, WorkflowEvent};
use moor_core::gate::{status, wait, GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxGateLines, Boxes, Events};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, OptionalExtension as _, Tx};
use crate::store::StoreError;

/// Writes the `gate_waiting` event of Box `id` when the line just appended moved its gates to a
/// wait for a person.
pub(super) fn note(tx: &Tx<'_>, id: BoxId, at: Ts) -> Result<(), StoreError> {
    let Some((gates, lines)) = load(tx, id)? else {
        return Ok(());
    };
    let Some((_, before)) = lines.split_last() else {
        return Ok(());
    };
    if status(&gates, before) == status(&gates, &lines) {
        return Ok(());
    }
    write(tx, id, at, (&gates, &lines))
}

/// Writes the `gate_waiting` event of Box `id` when its gates wait for a person and the wait has
/// none yet, whatever lines there are, none included.
pub(super) fn ensure(tx: &Tx<'_>, id: BoxId, at: Ts) -> Result<(), StoreError> {
    let Some((gates, lines)) = load(tx, id)? else {
        return Ok(());
    };
    let Some(waits) = wait(&gates, &lines) else {
        return Ok(());
    };
    let payloads: Vec<String> = tx
        .rows(
            Query::select()
                .column(Events::Payload)
                .from(Events::Table)
                .and_where(Expr::col(Events::Box).eq(id.get()))
                .and_where(Expr::col(Events::Kind).eq("gate_waiting")),
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<_, _>>()?;
    let noted = payloads.iter().any(|p| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(p) else {
            return false;
        };
        v.get("gate").and_then(serde_json::Value::as_str) == Some(waits.gate.as_str())
            && v.get("seq").and_then(serde_json::Value::as_u64) == Some(u64::from(waits.seq))
    });
    if noted {
        return Ok(());
    }
    write(tx, id, at, (&gates, &lines))
}

/// The gates of Box `id` and the lines written for them; none for a Box without gates.
fn load(tx: &Tx<'_>, id: BoxId) -> Result<Option<(Gates, Vec<GateLine>)>, StoreError> {
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
        return Ok(None);
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
    Ok(Some((gates, lines)))
}

/// Writes the event of the wait the gates are at, when they are at one.
fn write(
    tx: &Tx<'_>,
    id: BoxId,
    at: Ts,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<(), StoreError> {
    let Some(waits) = wait(gates, lines) else {
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
