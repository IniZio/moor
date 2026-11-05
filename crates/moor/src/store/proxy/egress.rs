//! Events about a Box's Proxy: the audit the shared egress Proxy drains, and what its keeper
//! records.

use moor_core::attention::{open_limits, Limit};
use moor_core::domain::event::Event;
use moor_core::BoxId;
use moor_schema::tables::{Events, Proxy};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{load_box, Store, StoreError};

impl Store {
    /// Append an `EgressRefused`, `EgressAborted`, `CredentialWithheld`, `ProxyCredentialFailed`,
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
                | Event::EgressAborted { .. }
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
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        insert_event(&tx, box_id, event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// Appends `event` for `box_id` while it has a proxy row; false when it has none.
    pub(crate) fn proxy_row_event(&self, box_id: BoxId, event: &Event) -> Result<bool, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let exists = tx
            .row(
                Query::select()
                    .column(Proxy::Box)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(false);
        }
        insert_event(&tx, box_id, event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }

    /// The Box's `agent_rate_limited`, `proxy_credential_rejected` and
    /// `proxy_credential_recovered` events, oldest first, as (kind, payload).
    pub(crate) fn proxy_limit_events(
        &self,
        box_id: BoxId,
    ) -> Result<Vec<(String, serde_json::Value)>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([Events::Kind, Events::Payload])
                .from(Events::Table)
                .and_where(Expr::col(Events::Box).eq(box_id.get()))
                .and_where(Expr::col(Events::Kind).is_in([
                    "agent_rate_limited",
                    "proxy_credential_rejected",
                    "proxy_credential_recovered",
                ]))
                .order_by(Events::Id, Order::Asc),
            |row| {
                let payload: String = row.get(1)?;
                Ok::<_, StoreError>((row.get::<_, String>(0)?, serde_json::from_str(&payload)?))
            },
        )?;
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
