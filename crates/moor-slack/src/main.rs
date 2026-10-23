//! Entry point of the moor Slack front daemon.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::Parser;
use moor_core::user::Identity;
use moor_slack::{Log, Settings, ToolSettings};

/// Command line of the daemon.
#[derive(Parser)]
#[command(
    name = "moor-slack",
    version,
    about = "Slack front for the moor controller"
)]
struct Args {
    /// Controller socket to dial; defaults to `$MOOR_FRONT_SOCKET`.
    #[arg(long, env = "MOOR_SLACK_SOCKET")]
    socket: Option<PathBuf>,
    /// The front's config file: Slack token files and channels.
    #[arg(long, env = "MOOR_SLACK_CONFIG")]
    config: PathBuf,
    /// Directory for the front's own state; defaults to `MOOR_FRONT_STATE_DIR`, which the
    /// controller sets when it starts the front, else `$XDG_STATE_HOME/moor-slack` or
    /// `$HOME/.local/state/moor-slack`.
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
    /// The Slack user the tools act for, as the controller names them: `slack:<team>/<user>`.
    #[arg(long = "as")]
    acting_as: String,
    /// The thread whose Boxes the tools act on, as `<channel>/<thread ts>`.
    #[arg(long)]
    thread: String,
    /// Most Boxes the thread has open at once.
    #[arg(long, default_value_t = 1)]
    max_open_boxes: u32,
    /// Set when the server runs again after its inherited descriptors were closed.
    #[arg(long, hide = true)]
    inherited_closed: bool,
}

/// The state directory given, else the one the controller gave the front, else the default
/// under the user's state home.
fn state_dir(given: Option<PathBuf>) -> Result<PathBuf, String> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = given.or_else(|| var("MOOR_FRONT_STATE_DIR")) {
        return Ok(dir);
    }
    var("XDG_STATE_HOME")
        .or_else(|| var("HOME").map(|h| h.join(".local").join("state")))
        .map(|base| base.join("moor-slack"))
        .ok_or_else(|| "set --state-dir, XDG_STATE_HOME or HOME".to_owned())
}

/// The socket to dial: the one given, else `MOOR_FRONT_SOCKET`, which the controller sets
/// when it starts the front.
fn dial_path(given: Option<PathBuf>, front: Option<OsString>) -> Result<PathBuf, String> {
    given
        .or_else(|| front.filter(|v| !v.is_empty()).map(PathBuf::from))
        .ok_or_else(|| "set --socket or MOOR_FRONT_SOCKET".to_owned())
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
    let identity = Identity::parse(&given.acting_as).map_err(|e| format!("--as: {e}"))?;
    moor_slack::serve_tools(&ToolSettings {
        state_dir: given.state_dir,
        socket: given.socket,
        identity,
        thread: given.thread,
        max_open_boxes: given.max_open_boxes,
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
    let stop = Arc::new(AtomicBool::new(false));
    let result = state_dir(args.state_dir).and_then(|state_dir| {
        let socket = dial_path(args.socket, std::env::var_os("MOOR_FRONT_SOCKET"))?;
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&stop))
            .map_err(|e| format!("handle SIGTERM: {e}"))?;
        let settings = Settings {
            socket,
            config: args.config,
            state_dir,
            home: std::env::var_os("HOME").map(PathBuf::from),
            user: std::env::var("USER").ok(),
            path: std::env::var_os("PATH"),
            proxy_port: std::env::var("MOOR_FRONT_PROXY_PORT")
                .ok()
                .and_then(|port| port.parse().ok()),
            proxy_ca_pem: std::env::var("MOOR_FRONT_PROXY_CA_PEM").ok(),
            clock: unix_now,
            stop,
        };
        moor_slack::run(&settings, &log)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "moor-slack: {msg}");
            ExitCode::FAILURE
        }
    }
}
