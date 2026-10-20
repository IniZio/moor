//! Proxy rows: the per-box nonce, reserved box port, readiness and the supervisor lease.
//!
//! The lease is an owner token plus a heartbeat timestamp. A pid alone is never trusted, because
//! pids get reused; another claimant may take over only once the heartbeat is older than the lease.

use std::fmt;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::seam::rng::Rng;
use moor_core::{BoxId, Secret};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use super::{load_box, to_u32, Store, StoreError};

/// Lowest box port a proxy may be given.
pub(crate) const BOX_PORT_MIN: u16 = 20000;
/// Highest box port a proxy may be given (inclusive).
pub(crate) const BOX_PORT_MAX: u16 = 39999;

/// Number of random draws before falling back to a linear scan.
const DRAWS: u32 = 64;

/// The two ends of a ready proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProxyPorts {
    /// Port the box connects to.
    pub(crate) box_port: u16,
    /// Port the proxy listens on at the host.
    pub(crate) host_port: u16,
}

/// The supervisor process a ready proxy runs in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadyProc<'a> {
    /// Supervisor process id.
    pub(crate) pid: u32,
    /// Handle of the running proxy.
    pub(crate) handle: &'a str,
    /// Host boot the supervisor runs in; `None` where the host has no boot id.
    pub(crate) boot_id: Option<&'a str>,
}

/// A stored proxy row.
pub(crate) struct ProxyRow {
    /// Owning box.
    pub(crate) box_id: BoxId,
    /// Secret the box presents to the proxy.
    pub(crate) nonce: Secret,
    /// Reserved box port.
    pub(crate) box_port: u16,
    /// Host port once the proxy is ready.
    pub(crate) host_port: Option<u16>,
    /// Proxy process id; never trusted without the lease.
    pub(crate) pid: Option<u32>,
    /// Handle of the running proxy.
    pub(crate) handle: Option<String>,
    /// Whether the proxy has reported ready.
    pub(crate) ready: bool,
    /// Token of the supervisor holding the lease.
    pub(crate) lease_owner: Option<String>,
    /// When the lease holder last proved it was alive.
    pub(crate) lease_heartbeat: Option<Ts>,
    /// Host boot the supervisor marked the row ready in.
    pub(crate) boot_id: Option<String>,
}

impl fmt::Debug for ProxyRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyRow")
            .field("box_id", &self.box_id)
            .field("nonce", &format_args!("<redacted>"))
            .field("box_port", &self.box_port)
            .field("host_port", &self.host_port)
            .field("pid", &self.pid)
            .field("handle", &self.handle)
            .field("ready", &self.ready)
            .field("lease_owner", &self.lease_owner)
            .field("lease_heartbeat", &self.lease_heartbeat)
            .field("boot_id", &self.boot_id)
            .finish()
    }
}

/// Outcome of a reservation.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reserve {
    /// The row is reserved on this box port.
    Reserved(u16),
    /// A supervisor holds a live lease on the row; nothing changed.
    Leased {
        /// The current owner.
        owner: String,
    },
}

impl Store {
    /// Reserve a box port for `box_id` and store `nonce`. Keeps the row's port if no other row
    /// holds it; otherwise draws from `rng`. An existing row is reset to unready with no
    /// host port, pid, handle or lease, unless a lease younger than `lease` is live.
    pub(crate) fn reserve(
        &self,
        box_id: BoxId,
        nonce: &Secret,
        rng: &dyn Rng,
        lease: Duration,
    ) -> Result<Reserve, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        let existing: Option<(i64, Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT box_port, lease_owner, lease_heartbeat FROM proxy WHERE box = ?",
                [box_id.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let port = if let Some((raw, owner, beat)) = existing {
            let port = to_port("box_port", raw)?;
            if held_by_other(&tx, box_id, port)? {
                return Err(StoreError::Corrupt(format!(
                    "box_port {port} held by two rows"
                )));
            }
            let window = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX);
            if let (Some(owner), Some(beat)) = (owner, beat) {
                if now.as_millis().saturating_sub(beat) < window {
                    return Ok(Reserve::Leased { owner });
                }
            }
            tx.execute(
                "UPDATE proxy SET nonce = ?, ready = 0, host_port = NULL, pid = NULL, handle = NULL, \
                 lease_owner = NULL, lease_heartbeat = NULL, boot_id = NULL, updated_at = ? WHERE box = ?",
                params![nonce.expose(), now.as_millis(), box_id.get()],
            )?;
            port
        } else {
            let port = free_port(&tx, rng)?;
            tx.execute(
                    "INSERT INTO proxy (box, nonce, box_port, ready, updated_at) VALUES (?, ?, ?, 0, ?)",
                    params![box_id.get(), nonce.expose(), port, now.as_millis()],
                )?;
            port
        };
        tx.commit()?;
        drop(conn);
        Ok(Reserve::Reserved(port))
    }

    /// The proxy row of `box_id`, if any.
    pub(crate) fn proxy(&self, box_id: BoxId) -> Result<Option<ProxyRow>, StoreError> {
        let conn = self.reader()?;
        conn.query_row(
            "SELECT box, nonce, box_port, host_port, pid, handle, ready, lease_owner, lease_heartbeat, boot_id \
             FROM proxy WHERE box = ?",
            [box_id.get()],
            raw_from_row,
        )
        .optional()?
        .map(ProxyRow::try_from)
        .transpose()
    }

    /// Every proxy row, by box ascending.
    pub(crate) fn proxies(&self) -> Result<Vec<ProxyRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT box, nonce, box_port, host_port, pid, handle, ready, lease_owner, lease_heartbeat, boot_id \
             FROM proxy ORDER BY box",
        )?;
        let rows = stmt.query_and_then([], |r| ProxyRow::try_from(raw_from_row(r)?))?;
        rows.collect()
    }

    /// Reset the row of `box_id` to unready with no host port, pid, handle or lease, keeping its
    /// nonce and box port, so the next reservation restarts it on the same port.
    pub(crate) fn expire_proxy(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let now = self.now();
        let conn = self.writer()?;
        let changed = conn.execute(
            "UPDATE proxy SET ready = 0, host_port = NULL, pid = NULL, handle = NULL, \
             lease_owner = NULL, lease_heartbeat = NULL, boot_id = NULL, updated_at = ? WHERE box = ?",
            params![now.as_millis(), box_id.get()],
        )?;
        drop(conn);
        Ok(changed > 0)
    }

    /// Delete the proxy row of `box_id`; returns whether a row was deleted.
    pub(crate) fn remove_proxy(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("DELETE FROM proxy WHERE box = ?", [box_id.get()])?;
        park::clear(&tx, box_id)?;
        tx.commit()?;
        drop(conn);
        Ok(changed > 0)
    }
}

/// A row as stored, before range checks.
struct RawRow {
    /// Box id column.
    box_id: i64,
    /// Nonce column.
    nonce: Vec<u8>,
    /// Box port column.
    box_port: i64,
    /// Host port column.
    host_port: Option<i64>,
    /// Pid column.
    pid: Option<i64>,
    /// Handle column.
    handle: Option<String>,
    /// Ready flag column.
    ready: i64,
    /// Lease owner column.
    lease_owner: Option<String>,
    /// Lease heartbeat column.
    lease_heartbeat: Option<i64>,
    /// Boot id column.
    boot_id: Option<String>,
}

/// Read a proxy row without range checks.
fn raw_from_row(r: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        box_id: r.get(0)?,
        nonce: r.get(1)?,
        box_port: r.get(2)?,
        host_port: r.get(3)?,
        pid: r.get(4)?,
        handle: r.get(5)?,
        ready: r.get(6)?,
        lease_owner: r.get(7)?,
        lease_heartbeat: r.get(8)?,
        boot_id: r.get(9)?,
    })
}

impl TryFrom<RawRow> for ProxyRow {
    type Error = StoreError;

    fn try_from(r: RawRow) -> Result<Self, StoreError> {
        Ok(Self {
            box_id: BoxId::new(r.box_id),
            nonce: Secret::new(r.nonce),
            box_port: to_port("box_port", r.box_port)?,
            host_port: r.host_port.map(|p| to_port("host_port", p)).transpose()?,
            pid: r.pid.map(|p| to_u32("proxy pid", p)).transpose()?,
            handle: r.handle,
            ready: r.ready != 0,
            lease_owner: r.lease_owner,
            lease_heartbeat: r.lease_heartbeat.map(Ts::from_millis),
            boot_id: r.boot_id,
        })
    }
}

/// Convert a stored integer to a port.
fn to_port(what: &str, v: i64) -> Result<u16, StoreError> {
    u16::try_from(v).map_err(|_| StoreError::Corrupt(format!("{what} {v} out of range")))
}

/// Whether a row other than `box_id`'s holds `port`.
fn held_by_other(conn: &Connection, box_id: BoxId, port: u16) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM proxy WHERE box_port = ? AND box <> ?",
            params![port, box_id.get()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Draw a free port from `rng`, falling back to a linear scan from the last draw.
fn free_port(conn: &Connection, rng: &dyn Rng) -> Result<u16, StoreError> {
    let span = u32::from(BOX_PORT_MAX - BOX_PORT_MIN) + 1;
    let at = |offset: u32| BOX_PORT_MIN + u16::try_from(offset % span).unwrap_or(0);
    let mut draw = 0;
    for _ in 0..DRAWS {
        draw = rng.u32() % span;
        let port = at(draw);
        if !port_taken(conn, port)? {
            return Ok(port);
        }
    }
    for step in 1..=span {
        let port = at(draw + step);
        if !port_taken(conn, port)? {
            return Ok(port);
        }
    }
    Err(StoreError::Invalid("no free box port".into()))
}

/// Whether any row holds `port`.
fn port_taken(conn: &Connection, port: u16) -> Result<bool, StoreError> {
    Ok(conn
        .query_row("SELECT 1 FROM proxy WHERE box_port = ?", [port], |_| Ok(()))
        .optional()?
        .is_some())
}

mod egress;
mod lease;
mod park;

pub(crate) use lease::{Fenced, Lease};
pub(crate) use park::{TunnelAck, TunnelRequest, TunnelWant};

#[cfg(test)]
mod tests;
