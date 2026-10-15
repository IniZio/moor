//! moor: per-person workflow layer over boxes. Library half of the `moor` binary.
#![forbid(unsafe_code)]

mod adapters;
pub mod cli;
/// Client for the controller socket.
pub mod client;
mod config_load;
/// The controller daemon: serves Requests on a local socket.
pub mod controller;
mod cx;
pub mod env;
pub mod error;
mod moor;
mod ops;
mod proxy;
mod store;
#[cfg(test)]
mod testing;
mod wire;

pub use crate::moor::Moor;
/// Run the credential proxy for a Task until a stop signal; returns the exit code.
pub use crate::wire::serve_proxy;
pub use moor_core;
