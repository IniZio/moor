//! moor: per-person workflow layer over boxes. Library half of the `moor` binary.
#![forbid(unsafe_code)]

mod adapters;
pub mod cli;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by start, added in a later change")
)]
mod cx;
pub mod env;
pub mod error;
mod moor;
mod ops;
mod store;
mod wire;

pub use crate::moor::Moor;
pub use moor_core;
