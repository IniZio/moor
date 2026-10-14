//! The supervisor lease on a proxy row and the writes fenced by it.
//!
//! Every write a supervisor makes to its row names the lease owner, so a supervisor that stalled
//! past its lease cannot overwrite the row of the one that took over.

use std::time::Duration;

use moor_core::domain::event::Event;
use moor_core::TaskId;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::ProxyPorts;
use crate::store::apply::insert_event;
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
        task: TaskId,
        owner: &str,
        lease: Duration,
    ) -> Result<Lease, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT lease_owner, lease_heartbeat FROM proxy WHERE task = ?",
                [task.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((current, beat)) = current else {
            return Err(StoreError::NotFound(task));
        };
        let window = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX);
        let free = match (&current, beat) {
            (Some(o), Some(beat)) if o != owner => now.saturating_sub(beat) >= window,
            _ => true,
        };
        let outcome = if free {
            tx.execute(
                "UPDATE proxy SET lease_owner = ?, lease_heartbeat = ? WHERE task = ?",
                params![owner, now, task.get()],
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
    pub(crate) fn heartbeat(&self, task: TaskId, owner: &str) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE proxy SET lease_heartbeat = ? WHERE task = ? AND lease_owner = ?",
            params![now, task.get(), owner],
        )?;
        if changed == 0 && !proxy_exists(&tx, task)? {
            return Err(StoreError::NotFound(task));
        }
        tx.commit()?;
        drop(conn);
        Ok(changed == 1)
    }

    /// Clear the lease if `owner` holds it; no-op otherwise.
    pub(crate) fn release(&self, task: TaskId, owner: &str) -> Result<(), StoreError> {
        let conn = self.writer()?;
        conn.execute(
            "UPDATE proxy SET lease_owner = NULL, lease_heartbeat = NULL WHERE task = ? AND lease_owner = ?",
            params![task.get(), owner],
        )?;
        drop(conn);
        Ok(())
    }

    /// Record the ports, pid and handle, mark the proxy ready and append `ProxyStarted` in one
    /// transaction, provided `owner` holds the lease.
    pub(crate) fn mark_ready(
        &self,
        task: TaskId,
        owner: &str,
        ports: ProxyPorts,
        pid: u32,
        handle: &str,
    ) -> Result<Fenced, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE proxy SET box_port = ?, host_port = ?, pid = ?, handle = ?, ready = 1, updated_at = ? \
             WHERE task = ? AND lease_owner = ?",
            params![
                ports.box_port,
                ports.host_port,
                pid,
                handle,
                now.as_millis(),
                task.get(),
                owner
            ],
        )?;
        if changed == 0 {
            if proxy_exists(&tx, task)? {
                return Ok(Fenced::LostLease);
            }
            return Err(StoreError::NotFound(task));
        }
        let event = Event::ProxyStarted {
            box_port: ports.box_port,
            host_port: ports.host_port,
            pid,
            handle: handle.to_owned(),
        };
        insert_event(&tx, task, &event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(Fenced::Applied)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the start operation reserves the row in a later change"
        )
    )]
    /// Append a `TunnelRestarted` event for `task`, provided `owner` holds the lease.
    pub(crate) fn tunnel_restarted(
        &self,
        task: TaskId,
        owner: &str,
        box_port: u16,
        reason: &str,
    ) -> Result<Fenced, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let holder: Option<Option<String>> = tx
            .query_row(
                "SELECT lease_owner FROM proxy WHERE task = ?",
                [task.get()],
                |r| r.get(0),
            )
            .optional()?;
        match holder {
            None => return Err(StoreError::NotFound(task)),
            Some(h) if h.as_deref() != Some(owner) => return Ok(Fenced::LostLease),
            Some(_) => {}
        }
        let event = Event::TunnelRestarted {
            box_port,
            reason: reason.to_owned(),
        };
        insert_event(&tx, task, &event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(Fenced::Applied)
    }
}

/// Whether `task` has a proxy row.
fn proxy_exists(conn: &Connection, task: TaskId) -> Result<bool, StoreError> {
    Ok(conn
        .query_row("SELECT 1 FROM proxy WHERE task = ?", [task.get()], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}
