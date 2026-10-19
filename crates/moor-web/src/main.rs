//! Entry point of the moor web front daemon.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;

use clap::Parser;
use moor_web::actions::{self, start::StartCtx};
use moor_web::auth::read_token;
use moor_web::client::Client;
use moor_web::db::Db;
use moor_web::http::start;
use moor_web::jobs::Jobs;
use moor_web::views;
use moor_web::watch::{self, RelayConfig};

/// Command line of the daemon.
#[derive(Parser)]
#[command(
    name = "moor-web",
    version,
    about = "Web front for the moor controller"
)]
struct Args {
    /// Controller socket to dial.
    #[arg(long, env = "MOOR_WEB_SOCKET")]
    socket: PathBuf,
    /// Loopback port to listen on; 0 picks a free one.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// File holding the access token.
    #[arg(long, env = "MOOR_WEB_TOKEN_FILE")]
    token_file: PathBuf,
    /// Directory for the front's own state; defaults to `$XDG_STATE_HOME/moor-web`
    /// or `$HOME/.local/state/moor-web`.
    #[arg(long, env = "MOOR_WEB_STATE_DIR")]
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
        .map(|base| base.join("moor-web"))
        .ok_or_else(|| "set --state-dir, XDG_STATE_HOME or HOME".to_owned())
}

/// Current time in unix seconds.
#[allow(
    clippy::disallowed_methods,
    reason = "the daemon's one wall-clock read, injected into the handlers"
)]
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Starts the daemon and parks the main thread.
fn run(args: Args) -> Result<(), String> {
    let token = read_token(&args.token_file)?;
    let dir = state_dir(args.state_dir)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let db = Db::open(&dir.join("web.db")).map_err(|e| e.to_string())?;
    let db = Arc::new(Mutex::new(db));
    let client = Client::new(args.socket);
    let jobs = Jobs::new();
    let ctx = StartCtx {
        client: client.clone(),
        db: db.clone(),
        jobs: jobs.clone(),
        state_dir: dir,
        clock: unix_now,
    };
    let handler = actions::start::handler(
        ctx.clone(),
        actions::end::handler(
            ctx,
            jobs.handler(views::handler(
                client.clone(),
                db.clone(),
                actions::handler(
                    client.clone(),
                    watch::handler(client, db, RelayConfig::default()),
                ),
            )),
        ),
    );
    let server = start(args.port, token, handler).map_err(|e| format!("listen: {e}"))?;
    writeln!(io::stdout(), "{}", server.url()).map_err(|e| e.to_string())?;
    loop {
        thread::park();
    }
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "moor-web: {msg}");
            ExitCode::FAILURE
        }
    }
}
