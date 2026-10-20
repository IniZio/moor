//! Proxy audit events recorded by the supervisor's audit drain.

use moor_core::attention::{open_limits, Limit};
use moor_core::domain::event::Event;
use moor_core::BoxId;
use rusqlite::TransactionBehavior;

use crate::store::apply::insert_event;
use crate::store::{load_box, Store, StoreError};

impl Store {
    /// Append an `EgressRefused`, `CredentialWithheld`, `ProxyCredentialFailed`,
    /// `AgentRateLimited`, `ProxyCredentialRejected` or `ProxyCredentialRecovered` event for `box_id`.
    ///
    /// Any other event kind is rejected, and so is a box that does not exist.
    pub(crate) fn record_proxy_event(
        &self,
        box_id: BoxId,
        event: &Event,
    ) -> Result<(), StoreError> {
        if !matches!(
            event,
            Event::EgressRefused { .. }
                | Event::CredentialWithheld { .. }
                | Event::ProxyCredentialFailed { .. }
                | Event::AgentRateLimited { .. }
                | Event::ProxyCredentialRejected { .. }
                | Event::ProxyCredentialRecovered { .. }
        ) {
            return Err(StoreError::Invalid(format!(
                "`{}` is not a proxy audit event",
                event.kind()
            )));
        }
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        insert_event(&tx, box_id, event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// The Box's `agent_rate_limited`, `proxy_credential_rejected` and
    /// `proxy_credential_recovered` events, oldest first, as (kind, payload).
    pub(crate) fn proxy_limit_events(
        &self,
        box_id: BoxId,
    ) -> Result<Vec<(String, serde_json::Value)>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT kind, payload FROM events WHERE box = ?1 AND kind IN \
             ('agent_rate_limited', 'proxy_credential_rejected', 'proxy_credential_recovered') \
             ORDER BY id",
        )?;
        let rows = stmt.query_and_then([box_id.get()], |row| {
            let payload: String = row.get(1)?;
            Ok::<_, StoreError>((row.get::<_, String>(0)?, serde_json::from_str(&payload)?))
        })?;
        rows.collect()
    }

    /// The Box's open limits (`open_limits` over [`Store::proxy_limit_events`]).
    pub(crate) fn open_limits(&self, box_id: BoxId) -> Result<Vec<Limit>, StoreError> {
        let events = self.proxy_limit_events(box_id)?;
        Ok(open_limits(events.iter().map(|(k, p)| (k.as_str(), p))))
    }
}

#[cfg(test)]
mod tests;
