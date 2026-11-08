//! `moor-hc`: keeps one hc machine per up moor Box and pipes hc's bytes into each Box.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use moor_client::oidc::{Console, REDIRECT_PORT};
use moor_client::session::{self, Store, Tokens};
use moor_client::Client;
use moor_hc::error::Error;
use moor_hc::hc::Cli;
use moor_hc::service::Service;
use moor_hc::via::Link;
use moor_hc::{exec, sync};

/// The command that signs moor-hc in, named when a sign-in is missing or ended.
const LOGIN: &str = "moor-hc login";

/// moor Boxes as hc machines.
#[derive(Debug, Parser)]
#[command(name = "moor-hc", version = option_env!("MOOR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))]
struct Args {
    /// The moor server, `https://<host>`; the only one you signed in to if left out.
    #[arg(long, env = "MOOR_HC_SERVER", global = true)]
    server: Option<String>,
    /// A controller socket on this computer, in place of a server.
    #[arg(long, env = "MOOR_HC_SOCKET", global = true, conflicts_with = "server")]
    socket: Option<String>,
    /// What to do.
    #[command(subcommand)]
    cmd: Cmd,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Sign in to a moor server in your browser, once.
    Login {
        /// The server, `https://<host>`.
        url: Option<String>,
    },
    /// Forget the saved sign-in.
    Logout,
    /// Keep one hc machine per up Box: add one as a Box comes up, remove it as the Box sleeps
    /// or closes. Machines moor-hc did not add are never touched.
    Sync {
        /// Seconds between passes.
        #[arg(long, default_value_t = 5)]
        every: u64,
        /// Make one pass and exit.
        #[arg(long)]
        once: bool,
        /// The hc binary.
        #[arg(long, default_value = "hc")]
        hc: PathBuf,
    },
    /// Run CMD in Box NAME with stdin, stdout and stderr passed byte for byte, as hc's `--via`
    /// needs; exits with CMD's exit code, or 255 when moor-hc fails.
    Exec {
        /// The Box.
        name: String,
        /// The command; one word is run as a shell line.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Print the user service that runs `moor-hc sync`, and how to start it.
    Service,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let done = match args.cmd {
        Cmd::Exec { ref name, ref cmd } => run_exec(&args, name, cmd.clone()),
        _ => run(&args).map(|()| ExitCode::SUCCESS),
    };
    done.unwrap_or_else(|e| {
        let _ = writeln!(io::stderr(), "moor-hc: {e}");
        ExitCode::from(255)
    })
}

/// Runs every subcommand but `exec`.
fn run(args: &Args) -> Result<(), Error> {
    match &args.cmd {
        Cmd::Login { url } => {
            let server = url.as_ref().or(args.server.as_ref()).ok_or_else(|| {
                Error::Usage("name the server: moor-hc login https://<host>".to_owned())
            })?;
            let now = session::unix_now();
            session::login(server, REDIRECT_PORT, &Store::user(), &mut Console, now)
                .map_err(Error::Usage)?;
            Ok(writeln!(
                io::stderr(),
                "Signed in to {}.",
                session::normalize(server)
            )?)
        }
        Cmd::Logout => {
            let Link::Server(server) = link(args)? else {
                return Err(Error::Usage("a socket has no sign-in".to_owned()));
            };
            let gone = session::logout(&Store::user(), &server).map_err(Error::Usage)?;
            let said = if gone {
                "Signed out."
            } else {
                "Not signed in."
            };
            Ok(writeln!(io::stderr(), "{said}")?)
        }
        Cmd::Sync { every, once, hc } => run_sync(args, *every, *once, hc),
        Cmd::Service => print_service(args),
        Cmd::Exec { .. } => Err(Error::Usage("exec runs on its own".to_owned())),
    }
}

/// The controller the command line names, else the only server signed in to.
fn link(args: &Args) -> Result<Link, Error> {
    if let Some(socket) = &args.socket {
        return Ok(Link::Socket(socket.clone()));
    }
    if let Some(server) = &args.server {
        return Ok(Link::Server(session::normalize(server)));
    }
    let saved = Store::user().all().map_err(Error::Usage)?;
    match saved.as_slice() {
        [one] => Ok(Link::Server(one.server.clone())),
        [] => Err(Error::Usage(format!(
            "not signed in: run `{LOGIN} https://<host>`"
        ))),
        _ => Err(Error::Usage(
            "signed in to several servers: pass --server https://<host>".to_owned(),
        )),
    }
}

/// A client dialing `link`.
fn client(link: &Link) -> Client {
    match link {
        Link::Socket(socket) => Client::new(socket),
        Link::Server(server) => Client::remote(
            server.clone(),
            Arc::new(Tokens::new(Store::user(), server).login_with(LOGIN)),
        ),
    }
}

/// This binary's absolute path, as via commands and the service name it.
fn exe() -> Result<String, Error> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

/// Runs `exec` and exits with the command's exit code.
fn run_exec(args: &Args, name: &str, cmd: Vec<String>) -> Result<ExitCode, Error> {
    let client = client(&link(args)?);
    let (mut out, mut err) = (io::stdout(), io::stderr());
    let exit = exec::run(&client, name, cmd, io::stdin(), &mut out, &mut err)?;
    Ok(ExitCode::from(
        exit.and_then(|c| u8::try_from(c).ok()).unwrap_or(255),
    ))
}

/// Syncs once, or every `every` seconds, logging changes and failures.
fn run_sync(args: &Args, every: u64, once: bool, hc: &std::path::Path) -> Result<(), Error> {
    let link = link(args)?;
    let (client, exe) = (client(&link), exe()?);
    let mut hc = Cli {
        bin: hc.to_path_buf(),
    };
    loop {
        match sync::once(&client, &mut hc, &exe, &link) {
            Ok(lines) => lines
                .iter()
                .try_for_each(|l| writeln!(io::stdout(), "{l}"))?,
            Err(e) if once => return Err(e),
            Err(e) => writeln!(io::stderr(), "moor-hc: sync: {e}")?,
        }
        if once {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(every.max(1)));
    }
}

/// Prints the user service for this computer.
fn print_service(args: &Args) -> Result<(), Error> {
    let link = link(args)?;
    let (flag, value) = link.flag();
    let path = std::env::var("PATH").unwrap_or_default();
    let service = Service {
        exe: exe()?,
        args: vec!["sync".to_owned(), flag.to_owned(), value.to_owned()],
        path,
    };
    let (file, how) = service.render(std::env::consts::OS);
    let mut out = io::stdout();
    write!(out, "{file}")?;
    Ok(writeln!(io::stderr(), "{how}")?)
}
