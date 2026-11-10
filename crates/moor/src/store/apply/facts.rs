//! The state an event leaves behind, written in the transaction that records it, so no decision
//! about a Box reads its events back.

use moor_core::domain::event::{Event, StallEvent};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxHands, BoxState, Launches, Proxy};
use sea_query::{Expr, ExprTrait as _, Func, Query, Value};

use super::sql::update;
use crate::store::db::{self, DbExt as _, Tx};
use crate::store::select::state_update;
use crate::store::StoreError;

/// What the note owed for a lost disk says, as `box_state.owed_note` holds it.
pub(in crate::store) const DISK_LOST: &str = r#"{"kind":"disk_lost"}"#;

/// Writes the state `e`, recorded for Box `id` at `now`, leaves on the Box's row, its live launch
/// or its Proxy row.
pub(super) fn follow(tx: &Tx<'_>, id: BoxId, e: &Event, now: Ts) -> Result<(), StoreError> {
    let at = Value::from(now.as_millis());
    let none = Value::BigInt(None);
    match e {
        Event::SessionStarted { .. } => {
            let hands = Query::select()
                .expr(Expr::col(BoxHands::Inc).max())
                .from(BoxHands::Table)
                .and_where(Expr::col(BoxHands::Box).eq(id.get()))
                .take();
            live(tx, id, Launches::HandsInc, db::sub(hands))?;
        }
        Event::Relaunched { cause, .. } => {
            if let Some(cause) = cause {
                live(tx, id, Launches::Cause, Expr::val(cause.as_str()))?;
            }
            update(
                tx,
                &mut state_update(id),
                &[(BoxState::RelaunchCappedAt, none)],
            )?;
        }
        Event::RelaunchCapped { .. } => {
            update(
                tx,
                &mut state_update(id),
                &[(BoxState::RelaunchCappedAt, at)],
            )?;
        }
        Event::DiskLost => {
            update(
                tx,
                &mut state_update(id),
                &[(BoxState::OwedNote, DISK_LOST.into())],
            )?;
        }
        Event::TunnelRestarted { .. } => {
            tx.run(
                Query::update()
                    .table(Proxy::Table)
                    .value(Proxy::RestartedAt, at)
                    .and_where(Expr::col(Proxy::Box).eq(id.get())),
            )?;
        }
        _ => return launch_facts(tx, id, e, now),
    }
    Ok(())
}

/// The state `e` leaves on Box `id`'s live launch: an open stall, an unreachable stretch, an exit
/// save.
fn launch_facts(tx: &Tx<'_>, id: BoxId, e: &Event, now: Ts) -> Result<(), StoreError> {
    let at = Value::from(now.as_millis());
    let none = Value::BigInt(None);
    match e {
        Event::Stall(StallEvent::AgentStalled { .. }) => {
            live(tx, id, Launches::StalledAt, Expr::val(at))?;
        }
        Event::Stall(StallEvent::AgentResumed { .. }) => {
            live(tx, id, Launches::StalledAt, Expr::val(none))?;
        }
        Event::AgentUnreachable { .. } => {
            let first = Func::coalesce([Expr::col(Launches::UnreachableSince), Expr::val(at)]);
            live(tx, id, Launches::UnreachableSince, first.into())?;
        }
        Event::AgentLinkRestored { .. } => {
            live(tx, id, Launches::UnreachableSince, Expr::val(none))?;
        }
        Event::TranscriptSaved { trigger, .. }
        | Event::TranscriptFailed { trigger, .. }
        | Event::TranscriptSkipped { trigger, .. }
            if trigger == "exit" =>
        {
            live(tx, id, Launches::ExitSaveAt, Expr::val(at))?;
        }
        _ => {}
    }
    Ok(())
}

/// Sets `col` of Box `id`'s live launch, its newest not ended, to `value`.
fn live(tx: &Tx<'_>, id: BoxId, col: Launches, value: Expr) -> Result<(), StoreError> {
    let newest = Query::select()
        .expr(Expr::col(Launches::Id).max())
        .from(Launches::Table)
        .and_where(Expr::col(Launches::Box).eq(id.get()))
        .and_where(Expr::col(Launches::EndedAt).is_null())
        .take();
    tx.run(
        Query::update()
            .table(Launches::Table)
            .value(col, value)
            .and_where(Expr::col(Launches::Id).eq(db::sub(newest))),
    )?;
    Ok(())
}
