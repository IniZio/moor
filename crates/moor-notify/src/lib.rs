//! moor-notify: a front that posts a user's Box events to a webhook, so a client that
//! started Boxes and walked away learns when one idles or fails without polling.
//!
//! It decides nothing: each POST carries the event as the controller stored it, plus the
//! fields `box_status` already shows.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use moor_client::Client;

pub mod config;
pub mod cursors;
pub mod deliver;
pub mod follow;
pub mod payload;
pub mod sign;

/// Where the front writes its log lines.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A log that writes `moor-notify: <line>` to stderr.
#[must_use]
pub fn stderr_log() -> Log {
    Arc::new(|msg: &str| {
        let _ = writeln!(io::stderr(), "moor-notify: {msg}");
    })
}

/// Follows every hook in the config at `config` through the controller's front socket
/// `socket`, keeping cursors under `state_dir` when given. Returns only on a startup error.
///
/// # Errors
/// The config, a hook's identity or the cursor file is unusable.
pub fn serve(
    socket: &Path,
    config: &Path,
    state_dir: Option<PathBuf>,
    log: &Log,
) -> Result<(), String> {
    let hooks = config::load(config)?;
    let shared = follow::Shared {
        cursors: Arc::new(cursors::Cursors::open(state_dir)?),
        retry: deliver::Retry::default(),
        log: Arc::clone(log),
    };
    let base = Client::new(socket);
    let mut running = Vec::new();
    for hook in hooks {
        let client = base.clone().acting_as(follow::identity(&hook)?);
        let shared = shared.clone();
        running.push(thread::spawn(move || follow::run(&client, &hook, &shared)));
    }
    for hook in running {
        let _ = hook.join();
    }
    Ok(())
}
