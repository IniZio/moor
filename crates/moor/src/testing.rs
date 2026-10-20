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

/// A Box name for tests; bare names are still accepted.
pub(crate) fn box_name(s: &str) -> moor_core::BoxName {
    moor_core::BoxName::parse_allow_bare(s).unwrap()
}
