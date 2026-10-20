//! The Slack front: a daemon that answers Slack over Socket Mode and drives Boxes through the
//! moor controller socket.

use std::sync::Arc;

#[expect(
    dead_code,
    reason = "edits, blocks and ephemeral posts come with status messages and prompts"
)]
mod api;
#[expect(
    dead_code,
    reason = "thread replies are parsed once the worker steers and closes Boxes"
)]
mod command;
pub mod config;
mod daemon;
mod db;
mod dial;
mod secret_file;
mod socket;
mod worker;

pub use daemon::{run, Settings};

/// Diagnostic sink for the daemon's log; each call is one line.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;
