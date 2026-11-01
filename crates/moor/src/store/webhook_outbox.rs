//! The webhook outbox: one delivery row per event and matching webhook, written with the event,
//! leased to a delivering front by `webhook-claim` and deleted by `webhook-report`.

use std::time::Duration;

use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{params, OptionalExtension as _, Transaction, TransactionBehavior};

use super::{Store, StoreError};

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

/// Queues event `event` of kind `kind` on Box `id` for every webhook of the Box's user that
/// names that kind, inside the event's own transaction.
pub(super) fn enqueue(
    tx: &Transaction<'_>,
    id: BoxId,
    event: i64,
    kind: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO webhook_deliveries (webhook, event) SELECT w.id, ?2 FROM user_webhooks w \
         JOIN boxes b ON b.user = w.user WHERE b.id = ?1 \
         AND EXISTS (SELECT 1 FROM json_each(w.events) WHERE value = ?3) ORDER BY w.id",
        params![id.get(), event, kind],
    )?;
    Ok(())
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
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: Vec<i64> = tx
            .prepare(
                "SELECT d.id FROM webhook_deliveries d WHERE d.id = \
                 (SELECT MIN(x.id) FROM webhook_deliveries x WHERE x.webhook = d.webhook) \
                 AND (d.leased_until IS NULL OR d.leased_until <= ?1) ORDER BY d.id LIMIT ?2",
            )?
            .query_map(params![now, limit], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut claimed = Vec::with_capacity(ids.len());
        for id in ids {
            tx.execute(
                "UPDATE webhook_deliveries SET leased_until = ? WHERE id = ?",
                params![until, id],
            )?;
            claimed.push(tx.query_row(
                "SELECT d.id, w.id, w.user, w.url, w.key, e.box, e.session, e.kind, e.payload, \
                 e.ts FROM webhook_deliveries d JOIN user_webhooks w ON w.id = d.webhook \
                 JOIN events e ON e.id = d.event WHERE d.id = ?",
                [id],
                |r| {
                    Ok(ClaimedDelivery {
                        delivery: r.get(0)?,
                        webhook: r.get(1)?,
                        user: UserId::new(r.get(2)?),
                        url: r.get(3)?,
                        sealed_key: r.get(4)?,
                        box_id: BoxId::new(r.get(5)?),
                        session: r.get(6)?,
                        kind: r.get(7)?,
                        payload: r.get(8)?,
                        ts: r.get(9)?,
                    })
                },
            )?);
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
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(webhook) = tx
            .query_row(
                "DELETE FROM webhook_deliveries WHERE id = ? RETURNING webhook",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(false);
        };
        match outcome {
            Ok(()) => tx.execute(
                "UPDATE user_webhooks SET last_ok_at = ?, failing_since = NULL, \
                 last_error = NULL WHERE id = ?",
                params![now, webhook],
            )?,
            Err(reason) => tx.execute(
                "UPDATE user_webhooks SET failing_since = COALESCE(failing_since, ?), \
                 last_error = ? WHERE id = ?",
                params![
                    now,
                    reason.chars().take(MAX_ERROR_CHARS).collect::<String>(),
                    webhook
                ],
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
            .prepare("SELECT webhook, event FROM webhook_deliveries ORDER BY id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;
