//! The Slack front: a daemon that answers Slack over Socket Mode and drives Boxes through the
//! moor controller socket.

use std::sync::Arc;

#[expect(
    dead_code,
    reason = "the replies and edits are posted once the worker handles mentions"
)]
mod api;
#[expect(
    dead_code,
    reason = "the worker parses mentions with these once it handles them"
)]
mod command;
pub mod config;
mod daemon;
mod dial;
mod secret_file;
mod socket;

pub use daemon::{run, Settings};

/// Diagnostic sink for the daemon's log; each call is one line.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;
