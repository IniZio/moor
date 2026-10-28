//! `moor-herdr`: shows moor Boxes as herdr workspaces and relays their agents' terminals.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use moor_client::Client;
use moor_herdr::auth::login::{self, Console};
use moor_herdr::auth::{self, Store, Tokens};
use moor_herdr::boxes::open_boxes;
use moor_herdr::error::Error;
use moor_herdr::herdr::Cli;
use moor_herdr::picker::{self, Prompt, Tty};
use moor_herdr::sync::{reconcile, shell_quote};
use moor_herdr::{panel, relay, setup, shell};

/// moor Boxes in herdr.
#[derive(Debug, Parser)]
#[command(name = "moor-herdr", version = option_env!("MOOR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))]
struct Args {
    /// The controller socket to dial.
    #[arg(long, env = "MOOR_HERDR_SOCKET", global = true)]
    socket: Option<PathBuf>,
    /// The moor server to dial, `https://<host>`; sign in with `moor-herdr login`.
    #[arg(long, env = "MOOR_HERDR_SERVER", global = true)]
    server: Option<String>,
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
    /// Sign in to the server in your browser.
    Login,
    /// Forget the saved sign-in.
    Logout,
    /// Print the herdr config snippet that colours Boxes and binds moor's keys.
    HerdrSetup {
        /// The default shell to name; the plugin's own `bin/shell.sh` if left out.
        #[arg(long)]
        shell: Option<String>,
    },
    /// herdr's default shell: a shell in the Box whose workspace the pane is in, else the
    /// user's own shell.
    Shell {
        /// herdr started the pane as a login shell.
        #[arg(long)]
        login: bool,
    },
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

/// Where the controller is: a socket on this machine, or a signed-in server.
enum Link {
    /// A controller's Unix socket.
    Socket(PathBuf),
    /// A moor server.
    Server(String),
}

impl Link {
    /// The link the command line and environment name; a server wins over a socket.
    fn of(args: &Args) -> Result<Self, Error> {
        match (&args.server, &args.socket) {
            (Some(server), _) => Ok(Self::Server(auth::normalize(server))),
            (None, Some(socket)) => Ok(Self::Socket(socket.clone())),
            (None, None) => Err(Error::Usage(
                "no controller: set MOOR_HERDR_SERVER (or pass --server), or MOOR_HERDR_SOCKET for a local socket"
                    .to_owned(),
            )),
        }
    }

    /// A client dialing this link.
    fn client(&self) -> Client {
        match self {
            Self::Socket(socket) => Client::new(socket),
            Self::Server(server) => Client::remote(
                server.clone(),
                Arc::new(Tokens::new(Store::new(auth::state_dir()), server)),
            ),
        }
    }

    /// The flags that make a child `moor-herdr` dial this link.
    fn flags(&self) -> String {
        match self {
            Self::Socket(socket) => format!("--socket {}", shell_quote(&socket.to_string_lossy())),
            Self::Server(server) => format!("--server {}", shell_quote(server)),
        }
    }

    /// Whether there is something to dial: a socket file, or any server.
    fn present(&self) -> bool {
        match self {
            Self::Socket(socket) => socket.exists(),
            Self::Server(_) => true,
        }
    }
}

/// Runs one subcommand.
fn run(args: &Args) -> Result<(), Error> {
    match &args.cmd {
        Cmd::HerdrSetup { shell } => Ok(write!(
            io::stdout(),
            "{}",
            setup::snippet(&shell.clone().unwrap_or_else(setup::shell_path))
        )?),
        Cmd::Shell { login } => {
            let link = Link::of(args).ok();
            shell::run(link.map(|l| (l.client(), l.present())), *login)
        }
        Cmd::Login => sign_in(args),
        Cmd::Logout => sign_out(),
        Cmd::Attach { name } => relay::run(&Link::of(args)?.client(), name),
        Cmd::Sync { every } => sync(&Link::of(args)?, *every),
        Cmd::New { workspace } => new(&Link::of(args)?, workspace),
        Cmd::Panel { workspace } => {
            let link = Link::of(args)?;
            let attach = attach_command(&std::env::current_exe()?, &link);
            panel::term::run(&mut panel::term::Live {
                client: link.client(),
                herdr: Cli::from_env(),
                workspace: workspace.clone(),
                attach: &attach,
            })
        }
    }
}

/// Signs in to the named server in the browser and saves the credentials.
fn sign_in(args: &Args) -> Result<(), Error> {
    let Some(server) = &args.server else {
        return Err(Error::Usage(
            "no server: pass --server https://<host> or set MOOR_HERDR_SERVER".to_owned(),
        ));
    };
    let store = Store::new(auth::state_dir());
    login::run(
        server,
        login::REDIRECT_PORT,
        &store,
        &mut Console,
        auth::unix_now(),
    )
    .map_err(Error::Usage)?;
    writeln!(io::stderr(), "Signed in to {}.", auth::normalize(server))?;
    Ok(())
}

/// Deletes the saved credentials.
fn sign_out() -> Result<(), Error> {
    let gone = Store::new(auth::state_dir()).delete()?;
    writeln!(
        io::stderr(),
        "{}",
        if gone {
            "Signed out."
        } else {
            "Not signed in."
        }
    )?;
    Ok(())
}

/// The shell command that runs a Box's pane.
fn attach_command(exe: &Path, link: &Link) -> impl Fn(&str) -> String {
    let prefix = format!(
        "exec {} {} attach",
        shell_quote(&exe.to_string_lossy()),
        link.flags()
    );
    move |name: &str| format!("{prefix} {}", shell_quote(name))
}

/// Runs the picker in herdr's popup, which closes with this process, so a failure waits for
/// Enter to be read first.
fn new(link: &Link, workspace: &str) -> Result<(), Error> {
    let attach = attach_command(&std::env::current_exe()?, link);
    let done = picker::run(
        &mut Tty,
        &link.client(),
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
fn sync(link: &Link, every: Option<u64>) -> Result<(), Error> {
    let client = link.client();
    let exe = std::env::current_exe()?;
    let attach = attach_command(&exe, link);
    let herdr = std::env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from);
    loop {
        // A follower that outlived its herdr server would sync nothing forever.
        if every.is_some() && herdr.as_deref().is_some_and(|p| !p.exists()) {
            return Ok(());
        }
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
