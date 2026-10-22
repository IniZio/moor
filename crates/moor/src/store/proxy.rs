//! Proxy rows: the reserved box port, readiness and the supervisor lease.
//!
//! The lease is an owner token plus a heartbeat timestamp. A pid alone is never trusted, because
//! pids get reused; another claimant may take over only once the heartbeat is older than the lease.

use std::fmt;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::seam::rng::Rng;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use super::{load_box, to_u32, Store, StoreError};

/// Lowest box port a proxy may be given.
pub(crate) const BOX_PORT_MIN: u16 = 20000;
/// Highest box port a proxy may be given (inclusive).
pub(crate) const BOX_PORT_MAX: u16 = 39999;
/// Lowest loopback port a Box's egress proxy may be given. The egress range sits below both the
/// box ports and Linux's default ephemeral range (32768-60999), so an outgoing connection in the
/// Box never holds it.
pub(crate) const EGRESS_PORT_MIN: u16 = 10000;
/// Highest egress port (inclusive).
pub(crate) const EGRESS_PORT_MAX: u16 = 19999;
const _: () = assert!(EGRESS_PORT_MAX < BOX_PORT_MIN);

/// Number of random draws before falling back to a linear scan.
const DRAWS: u32 = 64;

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
    /// Reserved box port.
    pub(crate) box_port: u16,
    /// The loopback port the Box's egress proxy listens on, kept for the life of the row.
    pub(crate) egress_port: u16,
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
            .field("box_port", &self.box_port)
            .field("egress_port", &self.egress_port)
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
    /// Reserve a box port for `box_id`. Keeps the row's ports if no other row
    /// holds them; otherwise draws both the box port and the egress port from `rng`. An existing row is reset to unready with no
    /// pid, handle or lease, unless a lease younger than `lease` is live.
    pub(crate) fn reserve(
        &self,
        box_id: BoxId,
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
                "UPDATE proxy SET ready = 0, pid = NULL, handle = NULL, \
                 lease_owner = NULL, lease_heartbeat = NULL, boot_id = NULL, updated_at = ? WHERE box = ?",
                params![now.as_millis(), box_id.get()],
            )?;
            port
        } else {
            let port = free_port(&tx, rng, BOX_PORTS)?;
            let egress = free_port(&tx, rng, EGRESS_PORTS)?;
            tx.execute(
                "INSERT INTO proxy (box, box_port, egress_port, ready, updated_at) VALUES (?, ?, ?, 0, ?)",
                params![box_id.get(), port, egress, now.as_millis()],
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
            "SELECT box, box_port, egress_port, pid, handle, ready, lease_owner, lease_heartbeat, boot_id \
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
            "SELECT box, box_port, egress_port, pid, handle, ready, lease_owner, lease_heartbeat, boot_id \
             FROM proxy ORDER BY box",
        )?;
        let rows = stmt.query_and_then([], |r| ProxyRow::try_from(raw_from_row(r)?))?;
        rows.collect()
    }

    /// Reset the row of `box_id` to unready with no pid, handle or lease, keeping its
    /// box port, so the next reservation restarts it on the same port.
    pub(crate) fn expire_proxy(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let now = self.now();
        let conn = self.writer()?;
        let changed = conn.execute(
            "UPDATE proxy SET ready = 0, pid = NULL, handle = NULL, \
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
    /// Box port column.
    box_port: i64,
    /// Egress port column.
    egress_port: i64,
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
        box_port: r.get(1)?,
        egress_port: r.get(2)?,
        pid: r.get(3)?,
        handle: r.get(4)?,
        ready: r.get(5)?,
        lease_owner: r.get(6)?,
        lease_heartbeat: r.get(7)?,
        boot_id: r.get(8)?,
    })
}

impl TryFrom<RawRow> for ProxyRow {
    type Error = StoreError;

    fn try_from(r: RawRow) -> Result<Self, StoreError> {
        Ok(Self {
            box_id: BoxId::new(r.box_id),
            box_port: to_port("box_port", r.box_port)?,
            egress_port: to_port("egress_port", r.egress_port)?,
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

/// A range of ports a proxy row reserves, and the column that holds them.
#[derive(Clone, Copy)]
struct Ports {
    /// Lowest port.
    min: u16,
    /// Highest port (inclusive).
    max: u16,
    /// The `proxy` column.
    column: &'static str,
    /// What the error names when the range is full.
    what: &'static str,
}

/// The box ports a Proxy tunnel listens on.
const BOX_PORTS: Ports = Ports {
    min: BOX_PORT_MIN,
    max: BOX_PORT_MAX,
    column: "box_port",
    what: "box port",
};

/// The loopback ports a Box's egress proxy listens on.
const EGRESS_PORTS: Ports = Ports {
    min: EGRESS_PORT_MIN,
    max: EGRESS_PORT_MAX,
    column: "egress_port",
    what: "egress port",
};

/// Draw a port of `ports` no row holds from `rng`, falling back to a linear scan from the last
/// draw.
fn free_port(conn: &Connection, rng: &dyn Rng, ports: Ports) -> Result<u16, StoreError> {
    let span = u32::from(ports.max - ports.min) + 1;
    let at = |offset: u32| ports.min + u16::try_from(offset % span).unwrap_or(0);
    let taken = |port: u16| -> Result<bool, StoreError> {
        let sql = format!("SELECT 1 FROM proxy WHERE {} = ?", ports.column);
        Ok(conn
            .query_row(&sql, [port], |_| Ok(()))
            .optional()?
            .is_some())
    };
    let mut draw = 0;
    for _ in 0..DRAWS {
        draw = rng.u32() % span;
        let port = at(draw);
        if !taken(port)? {
            return Ok(port);
        }
    }
    for step in 1..=span {
        let port = at(draw + step);
        if !taken(port)? {
            return Ok(port);
        }
    }
    Err(StoreError::Invalid(format!("no free {}", ports.what)))
}

mod egress;
mod lease;
mod park;

pub(crate) use lease::{Fenced, Lease};
pub(crate) use park::{TunnelAck, TunnelRequest, TunnelWant};

#[cfg(test)]
mod tests;
