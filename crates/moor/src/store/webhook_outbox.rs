//! The webhook outbox: one delivery row per event and matching webhook, written with the event,
//! leased to a delivering front by `webhook-claim` and deleted by `webhook-report`.

use std::time::Duration;

use moor_core::user::UserId;
use moor_core::view::user::WebhookState;
use moor_core::BoxId;
use moor_schema::tables::{Boxes, Events, UserWebhooks, WebhookDeliveries};
use sea_query::{Expr, ExprTrait as _, Func, Order, Query, SelectStatement};

use super::{Store, StoreError};
use crate::store::db::{self, DbExt as _, OptionalExtension as _, Tx};

/// Longest `last_error` kept, in characters.
const MAX_ERROR_CHARS: usize = 200;

/// One delivery a claim leased, with what its body is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedDelivery {
    /// The delivery's id.
    pub(crate) delivery: i64,
    /// The webhook it is for.
    pub(crate) webhook: i64,
    /// The webhook's user, whom its key is sealed for.
    pub(crate) user: UserId,
    /// Where to post.
    pub(crate) url: String,
    /// The webhook's sealed key.
    pub(crate) sealed_key: Vec<u8>,
    /// The receiver's header name and its sealed value, when the webhook has one.
    pub(crate) header: Option<(String, Vec<u8>)>,
    /// The event's id in the event log, which is its offset.
    pub(crate) event: i64,
    /// The event's Box.
    pub(crate) box_id: BoxId,
    /// The event's Session, when it had one.
    pub(crate) session: Option<i64>,
    /// The event kind.
    pub(crate) kind: String,
    /// The event payload, as stored.
    pub(crate) payload: String,
    /// When the event was written, in unix milliseconds.
    pub(crate) ts: i64,
}

/// Queues event `event` of kind `kind` on Box `id` for every active webhook of the Box's user
/// that names that kind, inside the event's own transaction.
pub(super) fn enqueue(tx: &Tx<'_>, id: BoxId, event: i64, kind: &str) -> Result<(), StoreError> {
    let hooks = tx.rows(
        Query::select()
            .column((UserWebhooks::Table, UserWebhooks::Id))
            .column((UserWebhooks::Table, UserWebhooks::Events))
            .from(UserWebhooks::Table)
            .inner_join(
                Boxes::Table,
                Expr::col((Boxes::Table, Boxes::User))
                    .equals((UserWebhooks::Table, UserWebhooks::User)),
            )
            .and_where(Expr::col((Boxes::Table, Boxes::Id)).eq(id.get()))
            .and_where(
                Expr::col((UserWebhooks::Table, UserWebhooks::State))
                    .eq(WebhookState::Active.as_str()),
            )
            .order_by((UserWebhooks::Table, UserWebhooks::Id), Order::Asc),
        |r| Ok::<_, db::Error>((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut named = Vec::new();
    for hook in hooks {
        let (webhook, events) = hook?;
        let events: Vec<String> = serde_json::from_str(&events)
            .map_err(|e| StoreError::Corrupt(format!("user_webhooks events: {e}")))?;
        if events.iter().any(|e| e == kind) {
            named.push(webhook);
        }
    }
    if named.is_empty() {
        return Ok(());
    }
    let mut insert = Query::insert();
    insert
        .into_table(WebhookDeliveries::Table)
        .columns([WebhookDeliveries::Webhook, WebhookDeliveries::Event]);
    for webhook in named {
        insert.values_panic([webhook.into(), event.into()]);
    }
    tx.run(insert)?;
    Ok(())
}

/// The oldest delivery of each webhook with no live lease at `now`, oldest first, up to `limit`.
fn next_deliveries(now: i64, limit: u32) -> SelectStatement {
    let oldest = Query::select()
        .expr(Expr::col(("x", WebhookDeliveries::Id)).min())
        .from_as(WebhookDeliveries::Table, "x")
        .and_where(
            Expr::col(("x", WebhookDeliveries::Webhook)).equals(("d", WebhookDeliveries::Webhook)),
        )
        .take();
    Query::select()
        .column(("d", WebhookDeliveries::Id))
        .from_as(WebhookDeliveries::Table, "d")
        .and_where(Expr::col(("d", WebhookDeliveries::Id)).eq(db::sub(oldest)))
        .and_where(
            Expr::col(("d", WebhookDeliveries::LeasedUntil))
                .is_null()
                .or(Expr::col(("d", WebhookDeliveries::LeasedUntil)).lte(now)),
        )
        .order_by(("d", WebhookDeliveries::Id), Order::Asc)
        .limit(u64::from(limit))
        .take()
}

/// Delivery `id` with the columns [`ClaimedDelivery`] is read from.
fn delivery_select(id: i64) -> SelectStatement {
    let wh = |c| (UserWebhooks::Table, c);
    let ev = |c| (Events::Table, c);
    Query::select()
        .column((WebhookDeliveries::Table, WebhookDeliveries::Id))
        .column(wh(UserWebhooks::Id))
        .column(wh(UserWebhooks::User))
        .column(wh(UserWebhooks::Url))
        .column(wh(UserWebhooks::Key))
        .column(ev(Events::Id))
        .column(ev(Events::Box))
        .column(ev(Events::Session))
        .column(ev(Events::Kind))
        .column(ev(Events::Payload))
        .column(ev(Events::Ts))
        .column(wh(UserWebhooks::Header))
        .column(wh(UserWebhooks::HeaderValue))
        .from(WebhookDeliveries::Table)
        .inner_join(
            UserWebhooks::Table,
            Expr::col(wh(UserWebhooks::Id))
                .equals((WebhookDeliveries::Table, WebhookDeliveries::Webhook)),
        )
        .inner_join(
            Events::Table,
            Expr::col(ev(Events::Id)).equals((WebhookDeliveries::Table, WebhookDeliveries::Event)),
        )
        .and_where(Expr::col((WebhookDeliveries::Table, WebhookDeliveries::Id)).eq(id))
        .take()
}

impl Store {
    /// Leases up to `limit` deliveries for `lease`: per webhook only its oldest, and only when
    /// no lease on it is live, so each webhook has at most one delivery in flight and keeps
    /// its order.
    pub(crate) fn claim_webhook_deliveries(
        &self,
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<ClaimedDelivery>, StoreError> {
        let now = self.now().as_millis();
        let until = now.saturating_add(i64::try_from(lease.as_millis()).unwrap_or(i64::MAX));
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let ids: Vec<i64> = tx
            .rows(next_deliveries(now, limit), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut claimed = Vec::with_capacity(ids.len());
        for id in ids {
            tx.run(
                Query::update()
                    .table(WebhookDeliveries::Table)
                    .value(WebhookDeliveries::LeasedUntil, until)
                    .and_where(Expr::col(WebhookDeliveries::Id).eq(id)),
            )?;
            claimed.push(tx.row(delivery_select(id), |r| {
                Ok(ClaimedDelivery {
                    delivery: r.get(0)?,
                    webhook: r.get(1)?,
                    user: UserId::new(r.get(2)?),
                    url: r.get(3)?,
                    sealed_key: r.get(4)?,
                    header: r.get::<_, Option<String>>(11)?.zip(r.get(12)?),
                    event: r.get(5)?,
                    box_id: BoxId::new(r.get(6)?),
                    session: r.get(7)?,
                    kind: r.get(8)?,
                    payload: r.get(9)?,
                    ts: r.get(10)?,
                })
            })?);
        }
        tx.commit()?;
        drop(writer);
        Ok(claimed)
    }

    /// Ends delivery `id` and records its outcome on its webhook: `Ok` clears the webhook's
    /// failure, `Err` holds why it failed. False when no such delivery is pending.
    pub(crate) fn report_webhook_delivery(
        &self,
        id: i64,
        outcome: Result<(), &str>,
    ) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let Some(webhook) = tx
            .row(
                Query::delete()
                    .from_table(WebhookDeliveries::Table)
                    .and_where(Expr::col(WebhookDeliveries::Id).eq(id))
                    .returning_col(WebhookDeliveries::Webhook),
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(false);
        };
        match outcome {
            Ok(()) => tx.run(
                Query::update()
                    .table(UserWebhooks::Table)
                    .value(UserWebhooks::LastOkAt, now)
                    .value(UserWebhooks::FailingSince, Option::<i64>::None)
                    .value(UserWebhooks::LastError, Option::<String>::None)
                    .and_where(Expr::col(UserWebhooks::Id).eq(webhook)),
            )?,
            Err(reason) => tx.run(
                Query::update()
                    .table(UserWebhooks::Table)
                    .value(
                        UserWebhooks::FailingSince,
                        Func::coalesce([Expr::col(UserWebhooks::FailingSince), Expr::value(now)]),
                    )
                    .value(
                        UserWebhooks::LastError,
                        reason.chars().take(MAX_ERROR_CHARS).collect::<String>(),
                    )
                    .and_where(Expr::col(UserWebhooks::Id).eq(webhook)),
            )?,
        };
        tx.commit()?;
        drop(writer);
        Ok(true)
    }

    /// Every pending delivery as (webhook, event), oldest first.
    #[cfg(test)]
    pub(crate) fn webhook_deliveries(&self) -> Result<Vec<(i64, i64)>, StoreError> {
        Ok(self
            .reader()?
            .rows(
                Query::select()
                    .columns([WebhookDeliveries::Webhook, WebhookDeliveries::Event])
                    .from(WebhookDeliveries::Table)
                    .order_by(WebhookDeliveries::Id, Order::Asc),
                |r| Ok::<_, db::Error>((r.get(0)?, r.get(1)?)),
            )?
            .collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;
