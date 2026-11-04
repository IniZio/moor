//! The supervisor lease on a proxy row and the writes fenced by it.
//!
//! Every write a supervisor makes to its row names the lease owner, so a supervisor that stalled
//! past its lease cannot overwrite the row of the one that took over.

use std::time::Duration;

use moor_core::domain::event::Event;
use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use super::ReadyProc;
use crate::store::apply::insert_event;
use crate::store::db::{Db, DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// Outcome of a lease claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lease {
    /// The caller now holds the lease.
    Claimed,
    /// Another owner holds a live lease.
    Held {
        /// The current owner.
        owner: String,
    },
}

/// Outcome of a write fenced by the lease.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fenced {
    /// The write applied.
    Applied,
    /// The caller no longer holds the lease; nothing was written.
    LostLease,
}

impl Store {
    /// Take the lease when it is free, already `owner`'s, or its heartbeat is at least `lease` old.
    pub(crate) fn claim_lease(
        &self,
        box_id: BoxId,
        owner: &str,
        lease: Duration,
    ) -> Result<Lease, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let current: Option<(Option<String>, Option<i64>)> = tx
            .row(
                Query::select()
                    .columns([Proxy::LeaseOwner, Proxy::LeaseHeartbeat])
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((current, beat)) = current else {
            return Err(StoreError::NotFound(box_id));
        };
        let window = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX);
        let free = match (&current, beat) {
            (Some(o), Some(beat)) if o != owner => now.saturating_sub(beat) >= window,
            _ => true,
        };
        let outcome = if free {
            tx.run(
                Query::update()
                    .table(Proxy::Table)
                    .value(Proxy::LeaseOwner, owner)
                    .value(Proxy::LeaseHeartbeat, now)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
            )?;
            Lease::Claimed
        } else {
            Lease::Held {
                owner: current.unwrap_or_default(),
            }
        };
        tx.commit()?;
        drop(conn);
        Ok(outcome)
    }

    /// Refresh the heartbeat; `Ok(false)` if `owner` no longer holds the lease.
    pub(crate) fn heartbeat(&self, box_id: BoxId, owner: &str) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let changed = tx.run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::LeaseHeartbeat, now)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::LeaseOwner).eq(owner)),
        )?;
        if changed == 0 && !proxy_exists(&tx, box_id)? {
            return Err(StoreError::NotFound(box_id));
        }
        tx.commit()?;
        drop(conn);
        Ok(changed == 1)
    }

    /// Clear the lease if `owner` holds it; no-op otherwise.
    pub(crate) fn release(&self, box_id: BoxId, owner: &str) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::LeaseOwner, Option::<String>::None)
                .value(Proxy::LeaseHeartbeat, Option::<i64>::None)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::LeaseOwner).eq(owner)),
        )?;
        drop(conn);
        Ok(())
    }

    /// Record the box port, pid, handle and host boot id, mark the proxy ready and append
    /// `ProxyStarted` in one transaction, provided `owner` holds the lease.
    pub(crate) fn mark_ready(
        &self,
        box_id: BoxId,
        owner: &str,
        box_port: u16,
        proc: ReadyProc<'_>,
    ) -> Result<Fenced, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let changed = tx.run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::BoxPort, box_port)
                .value(Proxy::Pid, proc.pid)
                .value(Proxy::Handle, proc.handle)
                .value(Proxy::Ready, 1)
                .value(Proxy::BootId, proc.boot_id)
                .value(Proxy::UpdatedAt, now.as_millis())
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::LeaseOwner).eq(owner)),
        )?;
        if changed == 0 {
            if proxy_exists(&tx, box_id)? {
                return Ok(Fenced::LostLease);
            }
            return Err(StoreError::NotFound(box_id));
        }
        let event = Event::ProxyStarted {
            box_port,
            pid: proc.pid,
            handle: proc.handle.to_owned(),
        };
        insert_event(&tx, box_id, &event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(Fenced::Applied)
    }

    /// Append a `TunnelRestarted` event for `box_id`, provided `owner` holds the lease.
    pub(crate) fn tunnel_restarted(
        &self,
        box_id: BoxId,
        owner: &str,
        box_port: u16,
        reason: &str,
    ) -> Result<Fenced, StoreError> {
        let event = Event::TunnelRestarted {
            box_port,
            reason: reason.to_owned(),
        };
        self.fenced_event(box_id, owner, &event)
    }

    /// Append `event` for `box_id`, provided `owner` holds the lease.
    pub(crate) fn fenced_event(
        &self,
        box_id: BoxId,
        owner: &str,
        event: &Event,
    ) -> Result<Fenced, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let holder: Option<Option<String>> = tx
            .row(
                Query::select()
                    .column(Proxy::LeaseOwner)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| r.get(0),
            )
            .optional()?;
        match holder {
            None => return Err(StoreError::NotFound(box_id)),
            Some(h) if h.as_deref() != Some(owner) => return Ok(Fenced::LostLease),
            Some(_) => {}
        }
        insert_event(&tx, box_id, event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(Fenced::Applied)
    }
}

/// Whether `box_id` has a proxy row.
fn proxy_exists(conn: &dyn Db, box_id: BoxId) -> Result<bool, StoreError> {
    Ok(conn
        .row(
            Query::select()
                .column(Proxy::Box)
                .from(Proxy::Table)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
