//! Proxy audit events recorded by the supervisor's audit drain.

use moor_core::attention::{open_limits, Limit};
use moor_core::domain::event::Event;
use moor_core::TaskId;
use rusqlite::TransactionBehavior;

use crate::store::apply::insert_event;
use crate::store::{load_task, Store, StoreError};

impl Store {
    /// Append an `EgressRefused`, `CredentialWithheld`, `ProxyCredentialFailed`,
    /// `AgentRateLimited`, `ProxyCredentialRejected` or `ProxyCredentialRecovered` event for `task`.
    ///
    /// Any other event kind is rejected, and so is a task that does not exist.
    pub(crate) fn record_proxy_event(&self, task: TaskId, event: &Event) -> Result<(), StoreError> {
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
        if load_task(&tx, task)?.is_none() {
            return Err(StoreError::NotFound(task));
        }
        insert_event(&tx, task, event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// The Task's `agent_rate_limited`, `proxy_credential_rejected` and
    /// `proxy_credential_recovered` events, oldest first, as (kind, payload).
    pub(crate) fn proxy_limit_events(
        &self,
        task: TaskId,
    ) -> Result<Vec<(String, serde_json::Value)>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT kind, payload FROM events WHERE task = ?1 AND kind IN \
             ('agent_rate_limited', 'proxy_credential_rejected', 'proxy_credential_recovered') \
             ORDER BY id",
        )?;
        let rows = stmt.query_and_then([task.get()], |row| {
            let payload: String = row.get(1)?;
            Ok::<_, StoreError>((row.get::<_, String>(0)?, serde_json::from_str(&payload)?))
        })?;
        rows.collect()
    }

    /// The Task's open limits (`open_limits` over [`Store::proxy_limit_events`]).
    pub(crate) fn open_limits(&self, task: TaskId) -> Result<Vec<Limit>, StoreError> {
        let events = self.proxy_limit_events(task)?;
        Ok(open_limits(events.iter().map(|(k, p)| (k.as_str(), p))))
    }
}

#[cfg(test)]
mod tests;
