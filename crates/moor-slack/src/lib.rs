//! The Slack front: a daemon that answers Slack over Socket Mode and drives Boxes through the
//! moor controller socket.

use std::sync::Arc;

mod account;
#[expect(
    dead_code,
    reason = "edits, blocks and ephemeral posts come with status messages and prompts"
)]
mod api;
mod command;
pub mod config;
mod coordinator;
mod credentials;
mod daemon;
mod db;
mod dial;
mod email;
mod error_text;
mod escape;
mod feed;
mod github;
mod identity;
mod login;
mod machine;
mod mcp;
mod profile;
mod prompt;
mod secret_file;
mod slash;
mod socket;
mod stop;
mod turns;
mod upload;
mod watchers;
mod worker;

pub use coordinator::claude::wiring::EGRESS_PROXY_VARS;
pub use coordinator::mcp::{close_inherited, serve_tools, ToolSettings};
pub use daemon::{run, Settings};

/// Diagnostic sink for the daemon's log; each call is one line.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;
