//! `moor-machine-agent`: pairs this computer with a moor server and keeps it linked.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use moor_client::link::lease::{Boot, Cache, Verb};
use moor_client::link::ops::Reply;
use moor_machine_agent::pool::{dial, lease, splice};
use moor_machine_agent::service::Manager;
use moor_machine_agent::up::{self, down, Runtime, Up};
use moor_machine_agent::{run_with, state_dir, update, Machine, Pool};

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
    /// Pairs this machine with its owner, checks its provider, and runs the agent as a user
    /// service that starts at login; rerun it to apply new flags.
    Up(UpFlags),
    /// Stops and removes the user service; the machine stays paired.
    Down,
    /// Keeps this paired machine linked to its server until stopped; the user service runs it.
    Run {
        /// Serve Boxes from a crabbox runtime.
        #[command(flatten)]
        pool: PoolFlags,
        /// Tell the server how many GB of disk caches may use; reported, and handed to crabbox as its cache store cap.
        #[arg(long, value_name = "GB", requires = "runtime")]
        cache_gb: Option<u32>,
        /// Run `exec` scripts the server sends; without it they are refused.
        #[arg(long, conflicts_with = "runtime")]
        allow_exec: bool,
        /// When the server needs another release, install it and restart in place.
        #[arg(long)]
        auto_update: bool,
    },
    /// Installs a release of this agent in place of the running one.
    Update {
        /// The release, such as `1.2.3`.
        #[arg(long)]
        version: String,
    },
    /// Does a lease verb on this host and prints the answer as JSON.
    Lease {
        /// warmup, info, pause, pause-publish, resume, stop or list.
        #[arg(value_parser = verb)]
        verb: Verb,
        /// `moor-<controller>-<id>`; none for list.
        slug: Option<String>,
        /// The cache volumes a warmup mounts, as the link carries them in JSON.
        #[arg(long, value_parser = cache)]
        cache: Option<Cache>,
        /// crabbox's machine class a warmup boots.
        #[arg(long)]
        class: Option<String>,
        /// The pool.
        #[command(flatten)]
        pool: PoolFlags,
    },
    /// Carries stdin and stdout to a lease's sshd, as an ssh `ProxyCommand`.
    Dial {
        /// `moor-<controller>-<id>`.
        slug: String,
        /// The pool.
        #[command(flatten)]
        pool: PoolFlags,
    },
}

/// What `up` takes.
#[derive(clap::Args)]
struct UpFlags {
    /// The moor server, such as `https://moor.example.com`.
    #[arg(long, env = "MOOR_SERVER")]
    server: String,
    /// The machine's name, unique among your machines; this host's name by default.
    #[arg(long)]
    name: Option<String>,
    /// Labels a Box can ask for, comma separated.
    #[arg(long, value_delimiter = ',')]
    labels: Vec<String>,
    /// Offer Boxes on this machine through this crabbox provider; the one crabbox is configured
    /// with by default.
    #[arg(long, value_name = "PROVIDER")]
    runtime: Option<String>,
    /// Host no Boxes; run the `exec` scripts the server sends for Boxes that attach this machine.
    #[arg(long, conflicts_with = "runtime")]
    allow_exec: bool,
    /// The most Boxes this machine holds at once; memory may allow fewer.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..))]
    max_boxes: u16,
    /// The crabbox program.
    #[arg(long, default_value = "crabbox")]
    crabbox: PathBuf,
}

/// Brings this machine to linked as `flags` say.
fn up_here(flags: UpFlags) -> ExitCode {
    let runtime = match (flags.allow_exec, flags.runtime) {
        (true, _) => Runtime::Exec,
        (false, Some(provider)) => Runtime::Provider(provider),
        (false, None) => Runtime::CrabboxDefault,
    };
    let home = Machine::from_env().home;
    let req = Up {
        server: flags.server,
        name: flags.name.unwrap_or_else(host_name),
        labels: flags.labels,
        max_boxes: flags.max_boxes,
        runtime,
        crabbox: flags.crabbox,
        home: home.clone(),
    };
    done(up::up(
        &req,
        &state_dir(),
        &Manager::here(&home),
        &mut |line| say(line),
    ))
}

/// The pool a host serves.
#[derive(clap::Args)]
struct PoolFlags {
    /// Serve VM leases from this crabbox provider, and nothing else.
    #[arg(long, value_name = "PROVIDER")]
    runtime: Option<String>,
    /// Refuse a warmup once this many leases exist, and tell the server this is the most.
    #[arg(long, requires = "runtime")]
    max_boxes: Option<usize>,
    /// Keep this many MiB of memory for the host; Boxes get the rest. Default: 4096 plus a tenth.
    #[arg(long, value_name = "MIB", requires = "runtime")]
    reserve_mib: Option<u64>,
    /// The crabbox program.
    #[arg(long, default_value = "crabbox", requires = "runtime")]
    crabbox: PathBuf,
}

impl PoolFlags {
    /// The pool, when one is named.
    fn pool(self, cache_gb: Option<u32>) -> Option<Pool> {
        Some(Pool {
            provider: self.runtime?,
            max: self.max_boxes,
            reserve_mib: self.reserve_mib,
            cache_gb,
            crabbox: self.crabbox,
        })
    }
}

/// A lease cache from its JSON.
fn cache(s: &str) -> Result<Cache, String> {
    serde_json::from_str(s).map_err(|e| format!("not a lease cache: {e}"))
}

/// A lease verb by its wire name.
fn verb(s: &str) -> Result<Verb, String> {
    serde_json::from_value(serde_json::Value::from(s))
        .map_err(|_| format!("{s:?} is not a lease verb"))
}

fn main() -> ExitCode {
    let ended = match Args::parse().command {
        Command::Up(flags) => return up_here(flags),
        Command::Down => {
            return done(down(
                &Manager::here(&Machine::from_env().home),
                &state_dir(),
            ));
        }
        Command::Run {
            pool,
            cache_gb,
            allow_exec,
            auto_update,
        } => run_here(pool, cache_gb, allow_exec, auto_update),
        Command::Update { version } => return update_here(&version),
        Command::Lease {
            verb,
            slug,
            cache,
            class,
            pool,
        } => return lease_here(pool, verb, slug.as_deref(), &Boot { cache, class }),
        Command::Dial { slug, pool } => {
            return on_pool(pool, |rt, home, pool| {
                rt.block_on(async {
                    let sshd = dial(&home, &pool, &slug, None).await?;
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

/// Keeps this machine linked until the server or the host stops it.
fn run_here(
    pool: PoolFlags,
    cache_gb: Option<u32>,
    allow_exec: bool,
    auto_update: bool,
) -> Result<(), String> {
    if let Some(provider) = &pool.runtime {
        apply_settings(provider);
    }
    let machine = Machine {
        pool: pool.pool(cache_gb),
        allow_exec,
        ..Machine::from_env()
    };
    let rt = runtime()?;
    rt.block_on(run_with(&state_dir(), machine, auto_update, &|line| {
        say(line);
    }))
}

/// Adds what warm starts need to the crabbox user config, saying what changed.
fn apply_settings(provider: &str) {
    up::apply_settings(&Machine::from_env().home, provider, &mut |line| say(line));
}

/// Installs release `version` in place of this executable and says what changed.
fn update_here(version: &str) -> ExitCode {
    match update(version) {
        Ok(done) => {
            say(&format!("updated from {} to {}", done.from, done.to));
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

/// A runtime for the agent's async work.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start: {e}"))
}

/// Does `verb` to lease `slug` here, with `boot` on a warmup, and prints the answer.
fn lease_here(flags: PoolFlags, verb: Verb, slug: Option<&str>, boot: &Boot) -> ExitCode {
    on_pool(flags, |rt, home, pool| {
        let reply = rt.block_on(lease(&home, &pool, verb, slug, None, boot));
        let line = serde_json::to_string(&reply).map_err(|e| e.to_string())?;
        let _ = writeln!(std::io::stdout(), "{line}");
        match reply {
            Reply::Refused { .. } => Err(String::new()),
            _ => Ok(()),
        }
    })
}

/// Runs `work` for the named pool in this account's home; an empty error was already shown.
fn on_pool(
    flags: PoolFlags,
    work: impl FnOnce(&tokio::runtime::Runtime, PathBuf, Pool) -> Result<(), String>,
) -> ExitCode {
    let Some(pool) = flags.pool(None) else {
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

/// Shows how a command ended: its last line, or why it failed.
fn done(ended: Result<String, String>) -> ExitCode {
    match ended {
        Ok(line) => {
            say(&line);
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&e);
            ExitCode::FAILURE
        }
    }
}

/// This host's name, the machine's name when none is given.
fn host_name() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}
