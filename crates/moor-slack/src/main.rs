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
}

fn main() -> ExitCode {
    let args = Args::parse();
    let log: Log = Arc::new(|line| {
        let _ = writeln!(io::stderr(), "moor-slack: {line}");
    });
    let settings = Settings {
        socket: args.socket,
        config: args.config,
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    match moor_slack::run(&settings, log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "moor-slack: {msg}");
            ExitCode::FAILURE
        }
    }
}
