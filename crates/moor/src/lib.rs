//! moor: per-person workflow layer over boxes. Library half of the `moor` binary.
#![forbid(unsafe_code)]

pub mod cli;
mod cx;
pub mod env;
pub mod error;
mod moor;
mod ops;

pub use crate::moor::Moor;
pub use moor_core;
