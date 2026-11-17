//! moor: per-person workflow layer over boxes. Library half of the `moor` binary.
#![forbid(unsafe_code)]

mod adapters;
mod blobs;
pub mod cli;
/// Client for the controller socket.
pub mod client;
mod config_load;
/// The controller daemon: serves Requests on a local socket.
pub mod controller;
mod cx;
/// First-run checks: `moor doctor`.
pub mod doctor;
pub mod env;
pub mod error;
mod failpoint;
mod front_dir;
mod keys;
mod ledger_dir;
pub mod link_dial;
pub mod listen_cmd;
pub mod mcp_relay;
mod moor;
pub mod node_add;
mod ops;
mod proxy;
mod runner_dir;
pub mod sign_in;
mod stamp;
mod store;
#[cfg(test)]
mod testing;
pub mod up;
mod vault;
mod wire;

/// The driver over machines that provide Boxes themselves, and how it reaches their runners; the
/// runner's tests drive it against a real runner.
pub use crate::adapters::machine::{box_term, Machines, Reach};
pub use crate::moor::Moor;
/// Run the credential proxy for a Box until a stop signal; returns the exit code.
/// Open the state store, which brings its schema to this build's latest migration; returns
/// the line to print, or why it failed.
pub use crate::wire::{copy_store, migrate_store};

pub use moor_core;

/// Runs `moor proxy serve-egress`, the shared egress Proxy, with the words after
/// `serve-egress`; returns the exit code.
///
/// # Errors
/// Why the Proxy could not start.
pub fn serve_egress_proxy(words: &[std::ffi::OsString]) -> Result<u8, String> {
    proxy::egress::run::main(words)
}
