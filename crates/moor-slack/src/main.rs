//! Entry point of the moor Slack front daemon.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use moor_slack::{Log, Settings, ToolSettings};

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

/// Command line of `moor-slack mcp`, the coordinator's tool server for one turn.
#[derive(Parser)]
#[command(name = "moor-slack mcp")]
struct McpArgs {
    /// The front's state dir, which holds `slack.db`.
    #[arg(long)]
    state_dir: PathBuf,
    /// Controller socket to dial.
    #[arg(long)]
    socket: PathBuf,
    /// The thread whose Boxes the tools act on, as `<channel>/<thread ts>`.
    #[arg(long)]
    thread: String,
    /// Set when the server runs again after its inherited descriptors were closed.
    #[arg(long, hide = true)]
    inherited_closed: bool,
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

/// Runs the tool server: closes inherited descriptors first, then serves until stdin ends.
fn mcp() -> Result<(), String> {
    let command_line: Vec<_> = std::env::args_os().collect();
    let given = McpArgs::parse_from(command_line.iter().skip(1));
    moor_slack::close_inherited(&command_line, given.inherited_closed)?;
    moor_slack::serve_tools(&ToolSettings {
        state_dir: given.state_dir,
        socket: given.socket,
        thread: given.thread,
        clock: unix_now,
    })
}

fn main() -> ExitCode {
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        return match mcp() {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                let _ = writeln!(io::stderr(), "moor-slack mcp: {msg}");
                ExitCode::FAILURE
            }
        };
    }
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
