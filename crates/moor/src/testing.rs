//! Test doubles and builders; compiled only under `cfg(test)`.

pub(crate) mod bridge;
pub(crate) mod clock;
pub(crate) mod cx;
pub(crate) mod driver;
pub(crate) mod engine;
pub(crate) mod forge;
pub(crate) mod proc;
pub(crate) mod proxy_host;
pub(crate) mod rng;
pub(crate) mod surface;

/// Turns the store at `db` back into the v4 file an older moor left, whose Proxy front listened on
/// host TCP: every proxy row gets that front's host port.
pub(crate) fn tcp_front_v4(db: &std::path::Path) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute_batch(
            "ALTER TABLE proxy ADD COLUMN nonce BLOB NOT NULL DEFAULT x'00';
             UPDATE proxy SET host_port = 4000;
             UPDATE meta SET v = '4' WHERE k = 'schema_version';",
        )
        .unwrap();
}

/// A Box name for tests; bare names are still accepted.
pub(crate) fn box_name(s: &str) -> moor_core::BoxName {
    moor_core::BoxName::parse_allow_bare(s).unwrap()
}
