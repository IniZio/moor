//! The rows a new launch cuts: sent, or maybe sent, to an older launch whose turn never ended.
//!
//! Such a row is reported, never sent again, since the cut turn may already have pushed or
//! commented and a second send would repeat that.

use moor_core::domain::event::{Event, InboxEvent, TurnOutcome};
use moor_core::domain::inbox::{cut_note, Delivery, InboxId, InboxKind};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};
use moor_schema::tables::BoxInbox;
use sea_query::{Expr, ExprTrait as _, Query};

use super::queue::steered_by;
use super::{one, rows, select, write_delivery};
use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, Tx};
use crate::store::StoreError;

/// Cuts Box `box_id`'s rows sent to a launch older than `launch`. The turn a sent row started
/// ended with its launch, so it gets `turn_done` as exited. A row confirmed by `restored`, the
/// time of the archive the new conversation came from, is in that conversation and is deleted;
/// so is a menu choice or interrupt, which meant something only on the screen it was sent to.
/// Any other row becomes, in place and so ahead of every row queued after it, a queued note
/// that quotes it, with a `steer_cut` event. A cut note is queued again as it was.
pub(in crate::store) fn name_cut(
    tx: &Tx<'_>,
    box_id: BoxId,
    launch: LaunchId,
    restored: Option<Ts>,
    now: Ts,
) -> Result<(), StoreError> {
    let q = select(box_id)
        .and_where(Expr::col(BoxInbox::Launch).is_not_null())
        .and_where(Expr::col(BoxInbox::Launch).lt(launch.get()))
        .take();
    for row in rows(tx, q)? {
        if matches!(row.delivery, Delivery::Sent { .. }) {
            end_turn(tx, box_id, row.id, now)?;
        }
        let held = match row.delivery {
            Delivery::Sent { at, .. } => restored.is_some_and(|r| at <= r.as_millis()),
            Delivery::Queued { .. } | Delivery::InDoubt { .. } => false,
        };
        if held || matches!(row.kind, InboxKind::Choice | InboxKind::Interrupt) {
            tx.run(
                Query::delete()
                    .from_table(BoxInbox::Table)
                    .cond_where(one(box_id, row.id)),
            )?;
            continue;
        }
        let text = if row.kind == InboxKind::Note {
            row.text
        } else {
            let cut = Event::Inbox(InboxEvent::SteerCut {
                inbox: row.id.get(),
                sender: row.sender,
            });
            insert_event(tx, box_id, &cut, now)?;
            cut_note(&row.text)
        };
        tx.run(
            Query::update()
                .table(BoxInbox::Table)
                .value(BoxInbox::Kind, InboxKind::Note.as_str())
                .value(BoxInbox::Text, text)
                .cond_where(one(box_id, row.id)),
        )?;
        let queued = Delivery::Queued { attempts: 0 };
        write_delivery(tx, box_id, row.id, queued.columns(0))?;
    }
    Ok(())
}

/// Records that the turn sent row `id` started ended with its agent.
fn end_turn(tx: &Tx<'_>, box_id: BoxId, id: InboxId, now: Ts) -> Result<(), StoreError> {
    let Some(steer) = steered_by(tx, box_id, id)? else {
        return Ok(());
    };
    let done = Event::TurnDone {
        steer,
        outcome: TurnOutcome::Exited,
        inbox: Some(id.get()),
    };
    insert_event(tx, box_id, &done, now)?;
    Ok(())
}
