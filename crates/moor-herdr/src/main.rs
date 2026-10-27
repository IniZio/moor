//! `moor-herdr`: shows moor Boxes as herdr workspaces and relays their agents' terminals.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use moor_client::Client;
use moor_herdr::boxes::open_boxes;
use moor_herdr::error::Error;
use moor_herdr::herdr::Cli;
use moor_herdr::sync::{reconcile, shell_quote};
use moor_herdr::{relay, setup};

/// moor Boxes in herdr.
#[derive(Debug, Parser)]
#[command(name = "moor-herdr", version)]
struct Args {
    /// The controller socket to dial.
    #[arg(long, env = "MOOR_HERDR_SOCKET", global = true)]
    socket: Option<PathBuf>,
    /// What to do.
    #[command(subcommand)]
    cmd: Cmd,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Make herdr's workspaces match the open Boxes.
    Sync {
        /// Keep syncing, this many seconds apart.
        #[arg(long)]
        every: Option<u64>,
    },
    /// Relay the agent's terminal of Box NAME, then show its card.
    Attach {
        /// The Box.
        name: String,
    },
    /// Print the herdr config snippet that colours Boxes and binds moor's keys.
    HerdrSetup,
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(io::stderr(), "moor-herdr: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs one subcommand.
fn run(args: &Args) -> Result<(), Error> {
    let socket = || {
        args.socket.clone().ok_or_else(|| {
            Error::Usage("no controller socket: pass --socket or set MOOR_HERDR_SOCKET".to_owned())
        })
    };
    match &args.cmd {
        Cmd::HerdrSetup => Ok(write!(io::stdout(), "{}", setup::snippet())?),
        Cmd::Attach { name } => relay::run(&Client::new(socket()?), name),
        Cmd::Sync { every } => sync(&socket()?, *every),
    }
}

/// Syncs once, or every `every` seconds, logging changes and failures.
fn sync(socket: &PathBuf, every: Option<u64>) -> Result<(), Error> {
    let client = Client::new(socket);
    let exe = std::env::current_exe()?;
    let prefix = format!(
        "exec {} --socket {} attach",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&socket.to_string_lossy())
    );
    let attach = |name: &str| format!("{prefix} {}", shell_quote(name));
    loop {
        let synced =
            open_boxes(&client).and_then(|boxes| reconcile(&mut Cli::from_env(), &boxes, &attach));
        let mut out = io::stdout().lock();
        match (synced, every) {
            (Ok(lines), _) => lines.iter().try_for_each(|l| writeln!(out, "{l}"))?,
            (Err(e), None) => return Err(e),
            (Err(e), Some(_)) => writeln!(io::stderr(), "moor-herdr: sync: {e}")?,
        }
        drop(out);
        match every {
            None => return Ok(()),
            Some(secs) => thread::sleep(Duration::from_secs(secs.max(1))),
        }
    }
}
