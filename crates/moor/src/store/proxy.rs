//! Proxy rows: each Box's reserved ports, the spec and CA its engine starts from, and its
//! egress token.

use moor_core::seam::clock::Ts;
use moor_core::seam::rng::Rng;
use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Order, Query, UpdateStatement};

use super::{load_box, Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension, Row};

/// The columns a proxy row is read with, in the order [`raw_from_row`] reads them.
const COLUMNS: [Proxy; 4] = [
    Proxy::Box,
    Proxy::BoxPort,
    Proxy::EgressPort,
    Proxy::ForwardPort,
];

/// Marks the row of `box_id` touched, with its egress token undelivered.
fn reset(box_id: BoxId, now: Ts) -> UpdateStatement {
    Query::update()
        .table(Proxy::Table)
        .value(Proxy::TokenDelivered, 0)
        .value(Proxy::UpdatedAt, now.as_millis())
        .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
        .take()
}

/// Marks the egress token of `box_id` undelivered, as when the Box gets a fresh disk.
pub(crate) fn forget_egress_delivery(conn: &dyn Db, box_id: BoxId) -> Result<(), StoreError> {
    conn.run(
        Query::update()
            .table(Proxy::Table)
            .value(Proxy::TokenDelivered, 0)
            .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
    )?;
    Ok(())
}

/// Tries of a reserve whose forward port another writer took between the draw and the write.
const RESERVE_TRIES: u32 = 3;

/// Whether `e` is a write that collided on the unique forward port.
fn forward_port_taken(e: &crate::store::db::Error) -> bool {
    e.unique_key()
        .is_some_and(|k| k.is(&Proxy::Table, &[Proxy::ForwardPort]))
}

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

/// A stored proxy row.
#[derive(Debug)]
pub(crate) struct ProxyRow {
    /// Owning box.
    pub(crate) box_id: BoxId,
    /// Reserved box port.
    pub(crate) box_port: u16,
    /// The loopback port the Box's egress proxy listens on, kept for the life of the row.
    pub(crate) egress_port: u16,
    /// The loopback port the Box's egress forwarder listens on, kept for the life of the row;
    /// none until a reserve gives a row from before it one.
    pub(crate) forward_port: Option<u16>,
}

impl Store {
    /// Reserve a box port for `box_id`; the port. Keeps the row's ports if no other row holds
    /// them; otherwise draws the box, egress and forward ports from `rng`. A row with no forward
    /// port is given one. An existing row's egress token is marked undelivered.
    pub(crate) fn reserve(&self, box_id: BoxId, rng: &dyn Rng) -> Result<u16, StoreError> {
        let mut tries = 0;
        loop {
            tries += 1;
            match self.reserve_once(box_id, rng) {
                Err(StoreError::Db(e)) if tries < RESERVE_TRIES && forward_port_taken(&e) => {}
                other => return other,
            }
        }
    }

    /// One try of [`Self::reserve`]; a forward port another writer took first fails it.
    fn reserve_once(&self, box_id: BoxId, rng: &dyn Rng) -> Result<u16, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        let existing: Option<i64> = tx
            .row(
                Query::select()
                    .column(Proxy::BoxPort)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| r.get(0),
            )
            .optional()?;
        let port = if let Some(raw) = existing {
            let port = to_port("box_port", raw)?;
            if held_by_other(&tx, box_id, port)? {
                return Err(StoreError::Corrupt(format!(
                    "box_port {port} held by two rows"
                )));
            }
            tx.run(reset(box_id, now))?;
            give_forward_port(&tx, box_id, rng)?;
            port
        } else {
            let port = free_port(&tx, rng, BOX_PORTS)?;
            let egress = free_port(&tx, rng, EGRESS_PORTS)?;
            tx.run(
                Query::insert()
                    .into_table(Proxy::Table)
                    .columns([
                        Proxy::Box,
                        Proxy::BoxPort,
                        Proxy::EgressPort,
                        Proxy::UpdatedAt,
                    ])
                    .values_panic([
                        box_id.get().into(),
                        port.into(),
                        egress.into(),
                        now.as_millis().into(),
                    ]),
            )?;
            give_forward_port(&tx, box_id, rng)?;
            port
        };
        tx.commit()?;
        drop(conn);
        Ok(port)
    }

    /// The proxy row of `box_id`, if any.
    pub(crate) fn proxy(&self, box_id: BoxId) -> Result<Option<ProxyRow>, StoreError> {
        let conn = self.reader()?;
        conn.row(
            Query::select()
                .columns(COLUMNS)
                .from(Proxy::Table)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
            raw_from_row,
        )
        .optional()?
        .map(ProxyRow::try_from)
        .transpose()
    }

    /// Every proxy row, by box ascending.
    pub(crate) fn proxies(&self) -> Result<Vec<ProxyRow>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns(COLUMNS)
                .from(Proxy::Table)
                .order_by(Proxy::Box, Order::Asc),
            |r| ProxyRow::try_from(raw_from_row(r)?),
        )?;
        rows.collect()
    }

    /// Delete the proxy row of `box_id`; returns whether a row was deleted.
    pub(crate) fn remove_proxy(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let changed = tx.run(
            Query::delete()
                .from_table(Proxy::Table)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
        )?;
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
    /// Forward port column.
    forward_port: Option<i64>,
}

/// Read a proxy row without range checks.
fn raw_from_row(r: &Row<'_>) -> crate::store::db::Result<RawRow> {
    Ok(RawRow {
        box_id: r.get(0)?,
        box_port: r.get(1)?,
        egress_port: r.get(2)?,
        forward_port: r.get(3)?,
    })
}

impl TryFrom<RawRow> for ProxyRow {
    type Error = StoreError;

    fn try_from(r: RawRow) -> Result<Self, StoreError> {
        Ok(Self {
            box_id: BoxId::new(r.box_id),
            box_port: to_port("box_port", r.box_port)?,
            egress_port: to_port("egress_port", r.egress_port)?,
            forward_port: r
                .forward_port
                .map(|v| to_port("forward_port", v))
                .transpose()?,
        })
    }
}

/// Convert a stored integer to a port.
fn to_port(what: &str, v: i64) -> Result<u16, StoreError> {
    u16::try_from(v).map_err(|_| StoreError::Corrupt(format!("{what} {v} out of range")))
}

/// Whether a row other than `box_id`'s holds `port`.
fn held_by_other(conn: &dyn Db, box_id: BoxId, port: u16) -> Result<bool, StoreError> {
    Ok(conn
        .row(
            Query::select()
                .column(Proxy::Box)
                .from(Proxy::Table)
                .and_where(Expr::col(Proxy::BoxPort).eq(port))
                .and_where(Expr::col(Proxy::Box).ne(box_id.get())),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

mod egress;
mod key;
mod ports;
use ports::{free_port, give_forward_port, BOX_PORTS, EGRESS_PORTS};
pub(crate) mod state;

#[cfg(test)]
mod tests;
