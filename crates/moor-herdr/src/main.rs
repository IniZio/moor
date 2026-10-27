//! `moor-herdr`: shows moor Boxes as herdr workspaces and relays their agents' terminals.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use moor_client::Client;
use moor_herdr::boxes::open_boxes;
use moor_herdr::error::Error;
use moor_herdr::herdr::Cli;
use moor_herdr::picker::{self, Prompt, Tty};
use moor_herdr::sync::{reconcile, shell_quote};
use moor_herdr::{panel, relay, setup};

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
    /// Ask where new work goes: a local worktree or a new Box.
    New {
        /// The workspace the key was pressed in.
        #[arg(long, env = "MOOR_HERDR_WORKSPACE")]
        workspace: String,
    },
    /// Show the open Boxes as a list with buttons to attach, wake and close them.
    Panel {
        /// The workspace the panel was opened in.
        #[arg(long, env = "MOOR_HERDR_WORKSPACE")]
        workspace: String,
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
        Cmd::New { workspace } => new(&socket()?, workspace),
        Cmd::Panel { workspace } => {
            let socket = socket()?;
            let attach = attach_command(&std::env::current_exe()?, &socket);
            panel::term::run(&mut panel::term::Live {
                client: Client::new(&socket),
                herdr: Cli::from_env(),
                workspace: workspace.clone(),
                attach: &attach,
            })
        }
    }
}

/// The shell command that runs a Box's pane.
fn attach_command(exe: &Path, socket: &Path) -> impl Fn(&str) -> String {
    let prefix = format!(
        "exec {} --socket {} attach",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&socket.to_string_lossy())
    );
    move |name: &str| format!("{prefix} {}", shell_quote(name))
}

/// Runs the picker in herdr's popup, which closes with this process, so a failure waits for
/// Enter to be read first.
fn new(socket: &Path, workspace: &str) -> Result<(), Error> {
    let attach = attach_command(&std::env::current_exe()?, socket);
    let done = picker::run(
        &mut Tty,
        &Client::new(socket),
        &mut Cli::from_env(),
        workspace,
        &attach,
    );
    if let Err(e) = &done {
        let _ = Tty.say(&format!("moor-herdr: {e}"));
        let _ = Tty.ask("Press Enter to close. ");
    }
    done
}

/// Syncs once, or every `every` seconds, logging changes and failures.
fn sync(socket: &Path, every: Option<u64>) -> Result<(), Error> {
    let client = Client::new(socket);
    let exe = std::env::current_exe()?;
    let attach = attach_command(&exe, socket);
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
