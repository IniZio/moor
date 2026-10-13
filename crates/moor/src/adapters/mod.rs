//! Adapters: concrete implementations of the `moor-core` seams.

#[cfg_attr(not(test), expect(dead_code, reason = "wired in wire.rs"))]
pub(crate) mod box_agent;
pub(crate) mod clock;
pub(crate) mod crabbox;
pub(crate) mod creds;
pub(crate) mod git;
pub(crate) mod herdr;
pub(crate) mod proc;
pub(crate) mod rng;
