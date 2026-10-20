//! Entry point of the moor Slack front daemon.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use moor_slack::{Log, Settings};

/// Command line of the daemon.
#[derive(Parser)]
#[command(
    name = "moor-slack",
    version,
    about = "Slack front for the moor controller"
)]
struct Args {
    /// Controller socket to dial.
    #[arg(long, env = "MOOR_SLACK_SOCKET")]
    socket: PathBuf,
    /// The front's config file: Slack token files, channels and users.
    #[arg(long, env = "MOOR_SLACK_CONFIG")]
    config: PathBuf,
    /// Directory for the front's own state; defaults to `$XDG_STATE_HOME/moor-slack`
    /// or `$HOME/.local/state/moor-slack`.
    #[arg(long, env = "MOOR_SLACK_STATE_DIR")]
    state_dir: Option<PathBuf>,
}

/// The state directory given, or the default under the user's state home.
fn state_dir(given: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = given {
        return Ok(dir);
    }
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("XDG_STATE_HOME")
        .or_else(|| var("HOME").map(|h| h.join(".local").join("state")))
        .map(|base| base.join("moor-slack"))
        .ok_or_else(|| "set --state-dir, XDG_STATE_HOME or HOME".to_owned())
}

/// Current time in unix seconds.
#[allow(
    clippy::disallowed_methods,
    reason = "the daemon's one wall-clock read, injected into the worker"
)]
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn main() -> ExitCode {
    let args = Args::parse();
    let log: Log = Arc::new(|line| {
        let _ = writeln!(io::stderr(), "moor-slack: {line}");
    });
    let result = state_dir(args.state_dir).and_then(|state_dir| {
        let settings = Settings {
            socket: args.socket,
            config: args.config,
            state_dir,
            home: std::env::var_os("HOME").map(PathBuf::from),
            user: std::env::var("USER").ok(),
            clock: unix_now,
        };
        moor_slack::run(&settings, log)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "moor-slack: {msg}");
            ExitCode::FAILURE
        }
    }
}
