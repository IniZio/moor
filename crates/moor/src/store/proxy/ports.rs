//! Drawing the ports a proxy row reserves.

use moor_core::seam::rng::Rng;
use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Cond, Expr, ExprTrait as _, Query};

use super::{BOX_PORT_MAX, BOX_PORT_MIN, EGRESS_PORT_MAX, EGRESS_PORT_MIN};
use crate::store::db::{Db, DbExt as _, OptionalExtension};
use crate::store::StoreError;

/// Number of random draws before falling back to a linear scan.
const DRAWS: u32 = 64;

/// A range of ports a proxy row reserves, and the column that holds them.
#[derive(Clone, Copy)]
pub(super) struct Ports {
    /// Lowest port.
    min: u16,
    /// Highest port (inclusive).
    max: u16,
    /// The `proxy` columns no two rows may share a port of.
    columns: &'static [Proxy],
    /// What the error names when the range is full.
    what: &'static str,
}

/// The box ports a Proxy tunnel listens on.
pub(super) const BOX_PORTS: Ports = Ports {
    min: BOX_PORT_MIN,
    max: BOX_PORT_MAX,
    columns: &[Proxy::BoxPort],
    what: "box port",
};

/// The loopback ports a Box's egress proxy listens on.
pub(super) const EGRESS_PORTS: Ports = Ports {
    min: EGRESS_PORT_MIN,
    max: EGRESS_PORT_MAX,
    columns: &[Proxy::EgressPort, Proxy::ForwardPort],
    what: "egress port",
};

/// Gives the row of `box_id` a forward port from the egress range when it has none, so the
/// forwarder never shares a port with any row's egress proxy or forwarder.
pub(super) fn give_forward_port(
    conn: &dyn Db,
    box_id: BoxId,
    rng: &dyn Rng,
) -> Result<(), StoreError> {
    let missing = conn
        .row(
            Query::select()
                .column(Proxy::Box)
                .from(Proxy::Table)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::ForwardPort).is_null()),
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if missing {
        let port = free_port(conn, rng, EGRESS_PORTS)?;
        conn.run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::ForwardPort, port)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
        )?;
    }
    Ok(())
}

/// Draw a port of `ports` no row holds from `rng`, falling back to a linear scan from the last
/// draw.
pub(super) fn free_port(conn: &dyn Db, rng: &dyn Rng, ports: Ports) -> Result<u16, StoreError> {
    let span = u32::from(ports.max - ports.min) + 1;
    let at = |offset: u32| ports.min + u16::try_from(offset % span).unwrap_or(0);
    let taken = |port: u16| -> Result<bool, StoreError> {
        Ok(conn
            .row(
                Query::select()
                    .column(Proxy::Box)
                    .from(Proxy::Table)
                    .cond_where(
                        ports
                            .columns
                            .iter()
                            .fold(Cond::any(), |any, col| any.add(Expr::col(*col).eq(port))),
                    ),
                |_| Ok(()),
            )
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
