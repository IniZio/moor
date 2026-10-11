//! Adapters: concrete implementations of the `moor-core` seams.

pub(crate) mod clock;
#[cfg(test)]
pub(crate) mod fake_driver;
#[cfg(test)]
pub(crate) mod fake_proc;
#[cfg(test)]
pub(crate) mod fake_surface;
#[expect(dead_code, reason = "wired through Cx by a later change")]
pub(crate) mod git;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired through Cx by a later change")
)]
pub(crate) mod proc;
