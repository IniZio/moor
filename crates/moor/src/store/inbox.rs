//! Each Box's inbox, `box_inbox`: texts for its agent, kept until the turn each started ends.
//!
//! Anyone may append a row without the Box's turn: the write check exempts inserts into this
//! table, and only them. Only the Box's owner, holding its turn, updates or deletes
//! rows, so senders never wait on a Box that is busy or asleep.

use moor_core::domain::event::{Event, TurnOutcome};
use moor_core::domain::inbox::{Delivery, DeliveryColumns, InboxId, InboxKind, SendMark};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};
use moor_schema::tables::{BoxInbox, BoxState, Events};
use sea_query::{Cond, Expr, ExprTrait as _, Order, Query, SelectStatement};
use serde_json::Value;

use super::apply::insert_event;
use super::event_from_row;
use super::init::parsed_opt;
use super::select::events_of;
use super::{Store, StoreError};
use crate::store::db::{Db, DbExt as _, Row, Tx};

/// One inbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InboxRow {
    /// Its id, in arrival order.
    pub(crate) id: InboxId,
    /// Who it is from.
    pub(crate) kind: InboxKind,
    /// The text for the agent.
    pub(crate) text: String,
    /// The person, front and conversation that sent it.
    pub(crate) sender: String,
    /// When it arrived.
    pub(crate) created_at: Ts,
    /// How far it got towards the agent.
    pub(crate) delivery: Delivery,
}

impl Store {
    /// Appends `text` from `sender` to Box `box_id`'s inbox, queued, without the Box's turn.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or the Box does not exist.
    pub(crate) fn enqueue(
        &self,
        box_id: BoxId,
        kind: InboxKind,
        text: &str,
        sender: &str,
    ) -> Result<InboxId, StoreError> {
        self.check_insert(&BoxInbox::Table, box_id);
        let id: i64 = self.writer().row(
            Query::insert()
                .into_table(BoxInbox::Table)
                .columns([
                    BoxInbox::Box,
                    BoxInbox::Kind,
                    BoxInbox::Text,
                    BoxInbox::Sender,
                    BoxInbox::CreatedAt,
                ])
                .values_panic([
                    box_id.get().into(),
                    kind.as_str().into(),
                    text.into(),
                    sender.into(),
                    self.now().as_millis().into(),
                ])
                .returning_col(BoxInbox::Id),
            |r| r.get(0),
        )?;
        Ok(InboxId::new(id))
    }

    /// Box `box_id`'s oldest inbox row, the one its owner delivers or waits on next.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or the row is corrupt.
    #[cfg_attr(not(test), expect(dead_code, reason = "the inbox pass reads it"))]
    pub(crate) fn next_for(&self, box_id: BoxId) -> Result<Option<InboxRow>, StoreError> {
        Ok(rows(&self.reader()?, select(box_id).limit(1).take())?
            .into_iter()
            .next())
    }

    /// Records that a send of row `id` to `launch` begins, before it is sent.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] when the row cannot be sent now, as
    /// [`Delivery::mark`] says, or [`StoreError`] when it is gone or the database fails.
    pub(crate) fn mark_sending(
        &self,
        box_id: BoxId,
        id: InboxId,
        launch: LaunchId,
    ) -> Result<Delivery, StoreError> {
        self.move_delivery(box_id, id, |d| d.mark(launch))
    }

    /// Records that the send of row `id` reached the agent now, with its terminal at `mark`,
    /// and appends `steered`, its audit event, in the same transaction; the event's id.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] unless the row is in doubt, or [`StoreError`] when it is
    /// gone or the database fails.
    pub(crate) fn confirm_sent(
        &self,
        box_id: BoxId,
        id: InboxId,
        mark: SendMark,
        steered: &Event,
    ) -> Result<(Delivery, i64), StoreError> {
        let now = self.now();
        let (next, event) = self.move_delivery_with(box_id, id, Some(steered), |d| {
            d.confirm(now.as_millis(), mark)
        })?;
        let event = event.ok_or_else(|| StoreError::Invalid("no steered event".to_owned()))?;
        Ok((next, event))
    }

    /// Records that the agent the send began to was gone, keeping the send's count.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] unless the row is in doubt, or [`StoreError`] when it is
    /// gone or the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "queued delivery reads it"))]
    pub(crate) fn rollback(&self, box_id: BoxId, id: InboxId) -> Result<Delivery, StoreError> {
        self.move_delivery(box_id, id, Delivery::rollback)
    }

    /// Undoes the mark of a send the agent refused before any key was typed.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] unless the row is in doubt, or [`StoreError`] when it is
    /// gone or the database fails.
    pub(crate) fn refuse(&self, box_id: BoxId, id: InboxId) -> Result<Delivery, StoreError> {
        self.move_delivery(box_id, id, Delivery::refused)
    }

    /// Deletes row `id` unless it was sent: the send it was queued for was refused or failed.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    pub(crate) fn withdraw(&self, box_id: BoxId, id: InboxId) -> Result<(), StoreError> {
        self.check_write(box_id);
        self.writer().run(
            Query::delete()
                .from_table(BoxInbox::Table)
                .cond_where(one(box_id, id))
                .and_where(Expr::col(BoxInbox::SentAt).is_null()),
        )?;
        Ok(())
    }

    /// Box `box_id`'s sent rows, oldest first: the turns its agent has not finished.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a row is corrupt.
    pub(crate) fn sent(&self, box_id: BoxId) -> Result<Vec<InboxRow>, StoreError> {
        let q = select(box_id)
            .and_where(Expr::col(BoxInbox::SentAt).is_not_null())
            .take();
        rows(&self.reader()?, q)
    }

    /// Ends the turn sent row `id` started: deletes the row and records `turn_done` for its
    /// `steered` event in one transaction, and makes the Box's archive due. Older sent rows went
    /// into the same prompt, so they are deleted too, with no `turn_done` of their own. Returns
    /// whether it did; a row already answered, or not sent, is left.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails, or [`StoreError::Corrupt`] when the row
    /// has no `steered` event.
    pub(crate) fn answer(
        &self,
        box_id: BoxId,
        id: InboxId,
        outcome: TurnOutcome,
    ) -> Result<bool, StoreError> {
        self.check_write(box_id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let deleted = tx.run(
            Query::delete()
                .from_table(BoxInbox::Table)
                .cond_where(one(box_id, id))
                .and_where(Expr::col(BoxInbox::SentAt).is_not_null()),
        )?;
        if deleted == 0 {
            return Ok(false);
        }
        tx.run(
            Query::delete()
                .from_table(BoxInbox::Table)
                .and_where(Expr::col(BoxInbox::Box).eq(box_id.get()))
                .and_where(Expr::col(BoxInbox::Id).lt(id.get()))
                .and_where(Expr::col(BoxInbox::SentAt).is_not_null()),
        )?;
        let steer = steered_by(&tx, box_id, id)?.ok_or_else(|| {
            StoreError::Corrupt(format!("inbox row {} has no steered event", id.get()))
        })?;
        let done = Event::TurnDone {
            steer,
            outcome,
            inbox: Some(id.get()),
        };
        insert_event(&tx, box_id, &done, now)?;
        tx.run(
            Query::update()
                .table(BoxState::Table)
                .value(BoxState::ArchiveDue, now.as_millis())
                .and_where(Expr::col(BoxState::Box).eq(box_id.get())),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }

    /// Box `box_id`'s rows sent, or in doubt, to a launch older than `live`, oldest first: the
    /// turns a lost VM or a dead agent cut. `live` is the Box's newest launch that has not ended.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a row is corrupt.
    #[cfg_attr(not(test), expect(dead_code, reason = "a new launch reads it"))]
    pub(crate) fn cut(&self, box_id: BoxId, live: LaunchId) -> Result<Vec<InboxRow>, StoreError> {
        let q = select(box_id)
            .and_where(Expr::col(BoxInbox::Launch).is_not_null())
            .and_where(Expr::col(BoxInbox::Launch).lt(live.get()))
            .take();
        rows(&self.reader()?, q)
    }

    /// Deletes every row of Box `box_id`, as its close does, and returns them oldest first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a row is corrupt.
    #[cfg_attr(not(test), expect(dead_code, reason = "close drops the inbox with it"))]
    pub(crate) fn drop_all(&self, box_id: BoxId) -> Result<Vec<InboxRow>, StoreError> {
        self.check_write(box_id);
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let dropped = rows(&tx, select(box_id))?;
        tx.run(
            Query::delete()
                .from_table(BoxInbox::Table)
                .and_where(Expr::col(BoxInbox::Box).eq(box_id.get())),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(dropped)
    }

    /// Moves row `id`'s delivery by `step` in one transaction, under the Box's turn.
    fn move_delivery(
        &self,
        box_id: BoxId,
        id: InboxId,
        step: impl FnOnce(Delivery) -> Result<Delivery, moor_core::domain::Illegal>,
    ) -> Result<Delivery, StoreError> {
        Ok(self.move_delivery_with(box_id, id, None, step)?.0)
    }

    /// Moves row `id`'s delivery by `step` and appends `event` in one transaction, under the
    /// Box's turn; the event's id.
    fn move_delivery_with(
        &self,
        box_id: BoxId,
        id: InboxId,
        event: Option<&Event>,
        step: impl FnOnce(Delivery) -> Result<Delivery, moor_core::domain::Illegal>,
    ) -> Result<(Delivery, Option<i64>), StoreError> {
        self.check_write(box_id);
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let q = select(box_id)
            .and_where(Expr::col(BoxInbox::Id).eq(id.get()))
            .take();
        let row = rows(&tx, q)?.into_iter().next().ok_or_else(|| {
            StoreError::Invalid(format!(
                "Box {} has no inbox row {}",
                box_id.get(),
                id.get()
            ))
        })?;
        let sends = match row.delivery {
            Delivery::InDoubt { attempts, .. } => attempts,
            Delivery::Queued { .. } | Delivery::Sent { .. } => 0,
        };
        let next = step(row.delivery)?;
        write_delivery(&tx, box_id, id, next.columns(sends))?;
        let event = event
            .map(|e| insert_event(&tx, box_id, e, self.now()))
            .transpose()?;
        tx.commit()?;
        drop(conn);
        Ok((next, event))
    }
}

/// Stores `c` as row `id`'s delivery.
fn write_delivery(
    tx: &Tx<'_>,
    box_id: BoxId,
    id: InboxId,
    c: DeliveryColumns,
) -> Result<(), StoreError> {
    tx.run(
        Query::update()
            .table(BoxInbox::Table)
            .value(BoxInbox::Attempts, c.attempts)
            .value(BoxInbox::Launch, c.launch)
            .value(BoxInbox::SentAt, c.sent_at)
            .value(BoxInbox::TurnBefore, c.turn_before)
            .value(BoxInbox::StatusBefore, c.status_before)
            .cond_where(one(box_id, id)),
    )?;
    Ok(())
}

/// The newest `steered` event of Box `box_id` that sent row `id`.
///
/// The payloads are read here rather than in SQL, so no database's JSON functions are needed.
fn steered_by(tx: &Tx<'_>, box_id: BoxId, id: InboxId) -> Result<Option<i64>, StoreError> {
    let q = events_of(box_id)
        .and_where(Expr::col(Events::Kind).eq("steered"))
        .order_by(Events::Id, Order::Desc)
        .take();
    for event in tx.rows(q, event_from_row)? {
        let event = event?;
        if event.payload.get("inbox").and_then(Value::as_i64) == Some(id.get()) {
            return Ok(Some(event.id));
        }
    }
    Ok(None)
}

/// Row `id` of Box `box_id`.
fn one(box_id: BoxId, id: InboxId) -> Cond {
    Cond::all()
        .add(Expr::col(BoxInbox::Box).eq(box_id.get()))
        .add(Expr::col(BoxInbox::Id).eq(id.get()))
}

/// Box `box_id`'s rows, oldest first, in [`inbox_row`]'s column order.
fn select(box_id: BoxId) -> SelectStatement {
    Query::select()
        .columns([
            BoxInbox::Id,
            BoxInbox::Kind,
            BoxInbox::Text,
            BoxInbox::Sender,
            BoxInbox::CreatedAt,
            BoxInbox::Attempts,
            BoxInbox::Launch,
            BoxInbox::SentAt,
            BoxInbox::TurnBefore,
            BoxInbox::StatusBefore,
        ])
        .from(BoxInbox::Table)
        .and_where(Expr::col(BoxInbox::Box).eq(box_id.get()))
        .order_by(BoxInbox::Id, Order::Asc)
        .take()
}

/// Every row `q`, a [`select`], reads through `db`.
fn rows(db: &dyn Db, q: SelectStatement) -> Result<Vec<InboxRow>, StoreError> {
    db.rows(q, inbox_row)?.collect()
}

/// Parses a row [`select`] read.
fn inbox_row(r: &Row<'_>) -> Result<InboxRow, StoreError> {
    let id = InboxId::new(r.get(0)?);
    let kind: String = r.get(1)?;
    let cols = DeliveryColumns {
        attempts: r.get(5)?,
        launch: r.get(6)?,
        sent_at: r.get(7)?,
        turn_before: r.get(8)?,
        status_before: r.get(9)?,
    };
    Ok(InboxRow {
        id,
        kind: parsed_opt("inbox kind", &kind, InboxKind::parse)?,
        text: r.get(2)?,
        sender: r.get(3)?,
        created_at: Ts::from_millis(r.get(4)?),
        delivery: Delivery::parse(cols).ok_or_else(|| {
            StoreError::Corrupt(format!("inbox row {} delivery {cols:?}", id.get()))
        })?,
    })
}

mod queue;

pub(super) use queue::send_with_launch;

#[cfg(test)]
mod tests;
