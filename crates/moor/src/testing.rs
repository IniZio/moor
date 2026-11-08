//! Test doubles and builders; compiled only under `cfg(test)`.

pub(crate) mod bridge;
pub(crate) mod clock;
pub(crate) mod cx;
pub(crate) mod db;
pub(crate) mod driver;
pub(crate) mod engine;
pub(crate) mod forge;
pub(crate) mod hc;
pub(crate) mod proc;
pub(crate) mod proxy_host;
pub(crate) mod rng;
pub(crate) mod surface;

/// A Box name for tests; bare names are still accepted.
pub(crate) fn box_name(s: &str) -> moor_core::BoxName {
    moor_core::BoxName::parse_allow_bare(s).unwrap()
}

/// The runner the tests' links come from.
pub(crate) fn runner() -> moor_core::request::RunnerAddr {
    moor_core::request::RunnerAddr::new("/run/moor/runner/runner.sock")
}

/// `links` as the whole report of [`runner`], which is not draining.
pub(crate) fn one_runner(
    links: Vec<moor_core::request::MachineLink>,
) -> moor_core::request::MachineLinksReq {
    moor_core::request::MachineLinksReq {
        runner: runner(),
        draining: false,
        links,
        box_memory: Vec::new(),
    }
}
