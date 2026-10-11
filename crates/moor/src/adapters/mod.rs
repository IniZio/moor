//! Adapters: concrete implementations of the `moor-core` seams.

pub(crate) mod clock;
pub(crate) mod crabbox;
#[cfg(test)]
pub(crate) mod fake_driver;
#[cfg(test)]
pub(crate) mod fake_proc;
#[cfg(test)]
pub(crate) mod fake_surface;
#[expect(dead_code, reason = "used by start, added in a later change")]
pub(crate) mod git;
pub(crate) mod herdr;
pub(crate) mod proc;
