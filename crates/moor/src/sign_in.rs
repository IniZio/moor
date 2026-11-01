//! `moor login`, `logout`, `status` and `token`: one sign-in per moor server, shared by every
//! local moor client through `moor_client::session`.

use std::ffi::OsString;
use std::sync::mpsc::Sender;

use clap::{Parser, Subcommand};
use moor_client::oidc::{Console, Ui, REDIRECT_PORT};
use moor_client::session::{self, normalize, Credentials, Store, Tokens};
use moor_client::TokenSource;

use crate::cli::Output;

/// The sign-in commands.
#[derive(Debug, Parser)]
#[command(name = "moor")]
struct Args {
    /// What to do.
    #[command(subcommand)]
    cmd: Cmd,
}

/// One sign-in command.
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Sign in to a moor server in the browser.
    Login {
        /// The server, as its origin or its /mcp address.
        #[arg(long, env = "MOOR_SERVER")]
        server: String,
        /// Print the address instead of opening a browser, and read the pasted return.
        #[arg(long)]
        no_browser: bool,
    },
    /// Sign out of a moor server and revoke the sign-in.
    Logout {
        /// The server, as its origin or its /mcp address.
        #[arg(long, env = "MOOR_SERVER")]
        server: String,
    },
    /// Show who is signed in, to one server or to each.
    Status {
        /// The server, as its origin or its /mcp address.
        #[arg(long, env = "MOOR_SERVER")]
        server: Option<String>,
    },
    /// Print a fresh access token, refreshing it when needed.
    Token {
        /// The server, as its origin or its /mcp address.
        #[arg(long, env = "MOOR_SERVER")]
        server: String,
        /// Print `{"Authorization":"Bearer …"}`, as a headers helper expects.
        #[arg(long)]
        headers: bool,
    },
}

/// A terminal that never opens a browser.
struct NoBrowser(Console);

impl Ui for NoBrowser {
    fn say(&mut self, line: &str) {
        self.0.say(line);
    }

    fn open(&mut self, _url: &str) {}

    fn pasted(&mut self, to: Sender<String>) {
        self.0.pasted(to);
    }
}

/// Runs a sign-in command; `None` when `args` names another command.
#[must_use]
pub fn run(args: &[OsString]) -> Option<Output> {
    let verb = args.get(1)?.to_str()?;
    if !matches!(verb, "login" | "logout" | "status" | "token") {
        return None;
    }
    let out = match Args::try_parse_from(args) {
        Ok(parsed) => match command(parsed.cmd) {
            Ok(stdout) => done(stdout, String::new(), 0),
            Err(why) => done(String::new(), format!("moor {verb}: {why}\n"), 1),
        },
        Err(e) if e.use_stderr() => done(String::new(), e.render().to_string(), 2),
        Err(e) => done(e.render().to_string(), String::new(), 0),
    };
    Some(out)
}

/// An [`Output`].
const fn done(stdout: String, stderr: String, code: u8) -> Output {
    Output {
        stdout,
        stderr,
        code,
    }
}

/// Runs `cmd`; what it prints on success.
fn command(cmd: Cmd) -> Result<String, String> {
    let store = Store::user();
    match cmd {
        Cmd::Login { server, no_browser } => {
            let mut console = Console;
            let mut quiet = NoBrowser(Console);
            let ui: &mut dyn Ui = if no_browser { &mut quiet } else { &mut console };
            let now = session::unix_now();
            let creds = session::login(&server, REDIRECT_PORT, &store, ui, now)?;
            Ok(format!("{}\n", shown(&creds, now)))
        }
        Cmd::Logout { server } => Ok(if session::logout(&store, &server)? {
            format!("Signed out of {}.\n", normalize(&server))
        } else {
            format!("{}: not signed in\n", normalize(&server))
        }),
        Cmd::Status { server } => status(&store, server.as_deref()),
        Cmd::Token { server, headers } => {
            let token = Tokens::new(store, &server).bearer()?;
            Ok(if headers {
                format!(
                    "{}\n",
                    serde_json::json!({"Authorization": format!("Bearer {token}")})
                )
            } else {
                format!("{token}\n")
            })
        }
    }
}

/// Who is signed in to `server`, or to each server when it is `None`.
fn status(store: &Store, server: Option<&str>) -> Result<String, String> {
    let now = session::unix_now();
    let lines: Vec<String> = match server {
        Some(server) => vec![store.read(server)?.map_or_else(
            || format!("{}: not signed in", normalize(server)),
            |c| shown(&c, now),
        )],
        None => store.all()?.iter().map(|c| shown(c, now)).collect(),
    };
    if lines.is_empty() {
        return Ok("not signed in to any moor server\n".to_owned());
    }
    Ok(lines.join("\n") + "\n")
}

/// One sign-in as a line, without its tokens.
fn shown(creds: &Credentials, now: i64) -> String {
    let who = creds.email.as_deref().unwrap_or("an unknown account");
    let left = creds.expires_at - now;
    let access = if left > 0 {
        format!("access token valid for {}m", left / 60)
    } else {
        "access token expired; it refreshes on next use".to_owned()
    };
    format!("{}: signed in as {who}; {access}", creds.server)
}
