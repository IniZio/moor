//! Adapters: concrete implementations of the `moor-core` seams.

pub(crate) mod clock;
pub(crate) mod crabbox;
#[cfg_attr(not(test), expect(dead_code, reason = "used by the env op"))]
pub(crate) mod creds;
pub(crate) mod git;
pub(crate) mod herdr;
pub(crate) mod proc;
pub(crate) mod rng;
