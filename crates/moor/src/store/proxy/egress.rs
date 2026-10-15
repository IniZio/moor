//! Proxy audit events recorded by the supervisor's audit drain.

use moor_core::domain::event::Event;
use moor_core::TaskId;
use rusqlite::TransactionBehavior;

use crate::store::apply::insert_event;
use crate::store::{load_task, Store, StoreError};

impl Store {
    /// Append an `EgressRefused`, `CredentialWithheld` or `ProxyCredentialFailed` event for `task`.
    ///
    /// Any other event kind is rejected, and so is a task that does not exist.
    pub(crate) fn record_proxy_event(&self, task: TaskId, event: &Event) -> Result<(), StoreError> {
        if !matches!(
            event,
            Event::EgressRefused { .. }
                | Event::CredentialWithheld { .. }
                | Event::ProxyCredentialFailed { .. }
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
}

#[cfg(test)]
mod tests;
