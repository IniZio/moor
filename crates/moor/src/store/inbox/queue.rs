//! The rows of a Box's inbox not yet sent: the queue its owner delivers, oldest first.

use moor_core::domain::event::{Event, SteerSent};
use moor_core::domain::inbox::{Delivery, InboxId, InboxKind, SendMark};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, LaunchId};
use moor_schema::tables::BoxInbox;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{one, rows, select, write_delivery};
use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, Tx};
use crate::store::{InboxRow, Store, StoreError};

impl Store {
    /// Box `box_id`'s rows not yet sent, oldest first: the queue its owner delivers.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a row is corrupt.
    #[cfg_attr(not(test), expect(dead_code, reason = "queued delivery reads it"))]
    pub(crate) fn unsent(&self, box_id: BoxId) -> Result<Vec<InboxRow>, StoreError> {
        let q = select(box_id)
            .and_where(Expr::col(BoxInbox::SentAt).is_null())
            .take();
        rows(&self.reader()?, q)
    }

    /// Every Box with a row not yet sent, by id.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "queued delivery reads it"))]
    pub(crate) fn boxes_with_unsent(&self) -> Result<Vec<BoxId>, StoreError> {
        let q = Query::select()
            .distinct()
            .column(BoxInbox::Box)
            .from(BoxInbox::Table)
            .and_where(Expr::col(BoxInbox::SentAt).is_null())
            .order_by(BoxInbox::Box, Order::Asc)
            .take();
        self.reader()?
            .rows(q, |r| Ok(BoxId::new(r.get(0)?)))?
            .collect()
    }

    /// Deletes row `id`, which moor could not confirm reached the agent, and records
    /// `steer_unconfirmed` with its text in the same transaction.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "queued delivery reads it"))]
    pub(crate) fn unconfirmed(&self, box_id: BoxId, id: InboxId) -> Result<(), StoreError> {
        self.check_write(box_id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let q = select(box_id)
            .and_where(Expr::col(BoxInbox::Id).eq(id.get()))
            .take();
        if let Some(row) = rows(&tx, q)?.into_iter().next() {
            tx.run(
                Query::delete()
                    .from_table(BoxInbox::Table)
                    .cond_where(one(box_id, id)),
            )?;
            let event = Event::SteerUnconfirmed {
                inbox: id.get(),
                text: row.text,
            };
            insert_event(&tx, box_id, &event, now)?;
        }
        tx.commit()?;
        drop(conn);
        Ok(())
    }
}

/// Sends Box `box_id`'s oldest queued prompt row holding `text` to `launch`, whose agent got it
/// as its first prompt, recording its `steered` event; nothing when no such row waits.
pub(in crate::store) fn send_with_launch(
    tx: &Tx<'_>,
    box_id: BoxId,
    (launch, session): (LaunchId, &str),
    text: &str,
    now: Ts,
) -> Result<(), StoreError> {
    let q = select(box_id)
        .and_where(Expr::col(BoxInbox::Kind).eq(InboxKind::Prompt.as_str()))
        .and_where(Expr::col(BoxInbox::Text).eq(text))
        .and_where(Expr::col(BoxInbox::Launch).is_null())
        .limit(1)
        .take();
    let Some(row) = rows(tx, q)?.into_iter().next() else {
        return Ok(());
    };
    // A fresh agent has ended no turn, so its first prompt's turn ends at its first.
    let mark = SendMark {
        turn_before: Some(0),
        status_before: None,
    };
    let marked = row.delivery.mark(launch)?;
    let sends = match marked {
        Delivery::InDoubt { attempts, .. } => attempts,
        Delivery::Queued { .. } | Delivery::Sent { .. } => 1,
    };
    let c = marked.confirm(now.as_millis(), mark)?.columns(sends);
    write_delivery(tx, box_id, row.id, c)?;
    let steered = Event::Steered(SteerSent {
        session: session.to_owned(),
        message: text.to_owned(),
        submit_retried: false,
        turn_before: mark.turn_before,
        status_before: None,
        inbox: Some(row.id.get()),
    });
    insert_event(tx, box_id, &steered, now)?;
    Ok(())
}
