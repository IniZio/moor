//! `moor-machine-agent`: enrolls this computer with a moor server and keeps it linked.

use std::io::Write;
use std::process::ExitCode;
use std::sync::mpsc::Sender;

use clap::{Parser, Subcommand};
use moor_client::oidc::{Console, Ui, REDIRECT_PORT};
use std::path::PathBuf;

use moor_client::link::lease::Verb;
use moor_client::link::ops::Reply;
use moor_machine_agent::pool::{dial, lease, splice};
use moor_machine_agent::{enroll, run, state_dir, Enroll, Machine, Pool};

/// Command line.
#[derive(Parser)]
#[command(name = "moor-machine-agent", version = option_env!("MOOR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))]
struct Args {
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// Subcommands.
#[derive(Subcommand)]
enum Command {
    /// Signs in as this machine's owner and enrolls it.
    Enroll {
        /// The moor server, such as `https://moor.example.com`.
        #[arg(long, env = "MOOR_SERVER")]
        server: String,
        /// The machine's name, unique among your machines.
        #[arg(long)]
        name: String,
        /// Labels a Box can ask for, comma separated.
        #[arg(long, value_delimiter = ',')]
        labels: Vec<String>,
        /// How many Boxes may hold this machine at once.
        #[arg(long, default_value_t = 1)]
        slots: u16,
        /// Offer Boxes on this machine through crabbox PROVIDER, such as `cloudhypervisor`.
        #[arg(long, value_name = "PROVIDER")]
        pool: Option<String>,
        /// Do not open a browser; paste the address the sign-in ends on.
        #[arg(long)]
        paste: bool,
        /// Make a new key instead of keeping this machine's.
        #[arg(long)]
        new_key: bool,
    },
    /// Keeps this enrolled machine linked to its server until stopped.
    Run {
        /// Serve a pool.
        #[command(flatten)]
        pool: PoolFlags,
    },
    /// Does a lease verb on this host and prints the answer as JSON.
    Lease {
        /// warmup, info, pause, resume, stop or list.
        #[arg(value_parser = verb)]
        verb: Verb,
        /// `moor-<id>`; none for list.
        slug: Option<String>,
        /// The pool.
        #[command(flatten)]
        pool: PoolFlags,
    },
    /// Carries stdin and stdout to a lease's sshd, as an ssh `ProxyCommand`.
    Dial {
        /// `moor-<id>`.
        slug: String,
        /// The pool.
        #[command(flatten)]
        pool: PoolFlags,
    },
}

/// The pool a host serves.
#[derive(clap::Args)]
struct PoolFlags {
    /// Serve VM leases from this crabbox provider, and nothing else.
    #[arg(long)]
    pool: Option<String>,
    /// Refuse a warmup once this many leases exist.
    #[arg(long, requires = "pool")]
    pool_max: Option<usize>,
    /// The crabbox program.
    #[arg(long, default_value = "crabbox", requires = "pool")]
    crabbox: PathBuf,
}

impl PoolFlags {
    /// The pool, when one is named.
    fn pool(self) -> Option<Pool> {
        Some(Pool {
            provider: self.pool?,
            max: self.pool_max,
            crabbox: self.crabbox,
        })
    }
}

/// A lease verb by its wire name.
fn verb(s: &str) -> Result<Verb, String> {
    serde_json::from_value(serde_json::Value::from(s))
        .map_err(|_| format!("{s:?} is not a lease verb"))
}

/// The console, with or without the browser.
struct Terminal {
    /// Opens the sign-in in the platform's browser.
    browser: bool,
}

impl Ui for Terminal {
    fn say(&mut self, line: &str) {
        Console.say(line);
    }

    fn open(&mut self, url: &str) {
        if self.browser {
            Console.open(url);
        }
    }

    fn pasted(&mut self, to: Sender<String>) {
        Console.pasted(to);
    }
}

fn main() -> ExitCode {
    let ended = match Args::parse().command {
        Command::Enroll {
            server,
            name,
            labels,
            slots,
            pool,
            paste,
            new_key,
        } => {
            let req = Enroll {
                server,
                name,
                labels,
                slots,
                pool,
                new_key,
            };
            return enroll_here(&req, paste);
        }
        Command::Run { pool } => {
            let machine = Machine {
                pool: pool.pool(),
                ..Machine::from_env()
            };
            runtime().and_then(|rt| rt.block_on(run(&state_dir(), machine, &|line| say(line))))
        }
        Command::Lease { verb, slug, pool } => {
            return on_pool(pool, |rt, home, pool| {
                let reply = rt.block_on(lease(&home, &pool, verb, slug.as_deref()));
                let line = serde_json::to_string(&reply).map_err(|e| e.to_string())?;
                let _ = writeln!(std::io::stdout(), "{line}");
                match reply {
                    Reply::Refused { .. } => Err(String::new()),
                    _ => Ok(()),
                }
            });
        }
        Command::Dial { slug, pool } => {
            return on_pool(pool, |rt, home, pool| {
                rt.block_on(async {
                    let sshd = dial(&home, &pool, &slug).await?;
                    let stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
                    splice(sshd, stdio).await;
                    Ok(())
                })
            });
        }
    };
    if let Err(e) = ended {
        say(&e);
    }
    ExitCode::FAILURE
}

/// A runtime for the agent's async work.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start: {e}"))
}

/// Runs `work` for the named pool in this account's home; an empty error was already shown.
fn on_pool(
    flags: PoolFlags,
    work: impl FnOnce(&tokio::runtime::Runtime, PathBuf, Pool) -> Result<(), String>,
) -> ExitCode {
    let Some(pool) = flags.pool() else {
        say("--pool names the crabbox provider to use");
        return ExitCode::FAILURE;
    };
    match runtime().and_then(|rt| work(&rt, Machine::from_env().home, pool)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.is_empty() {
                say(&e);
            }
            ExitCode::FAILURE
        }
    }
}

/// Writes `line` to stderr, where the agent logs.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "moor-machine-agent: {line}");
}

/// Enrolls this machine, signing in through the browser unless `paste`.
fn enroll_here(req: &Enroll, paste: bool) -> ExitCode {
    let mut ui = Terminal { browser: !paste };
    match enroll(req, &state_dir(), REDIRECT_PORT, &mut ui) {
        Ok(state) => {
            ui.say(&format!(
                "Enrolled {} as machine {} for {}.",
                state.name, state.machine_id, state.owner
            ));
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}
