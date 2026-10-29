//! `moor-machine-runner`: serve the machine runner MCP server on the host.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use moor_machine_runner::{serve, serve_agents, Agents, Config, Relay};

/// Serves the tools a Box uses to work on its attached machine.
#[derive(Parser)]
#[command(name = "moor-machine-runner")]
struct Args {
    /// Address to listen on; the Box's Proxy routes the service host here.
    #[arg(long, default_value = "127.0.0.1:7700")]
    listen: SocketAddr,
    /// Address to accept machine agents' links on, at `/machines/link`; none accepts no links.
    #[arg(long, value_name = "ADDR")]
    agents: Option<SocketAddr>,
    /// The Host the Box's Proxy passes through.
    #[arg(long, default_value = "machine.moor.internal")]
    service_host: String,
    /// The controller's operator socket.
    #[arg(long, env = "MOOR_SOCKET")]
    socket: PathBuf,
    /// Directory for the runner's per-Box host mirrors.
    #[arg(long)]
    state_dir: PathBuf,
    /// A capped resource each machine has, as NAME=CAP; repeat for more.
    #[arg(long = "resource", value_name = "NAME=CAP", value_parser = resource)]
    resources: Vec<(String, usize)>,
    /// Command run on the machine, in the Box's copy, before a Box's first `exec` or
    /// `machine_info`; its last stdout line is a JSON object of the Box's facts.
    #[arg(long, value_name = "CMD")]
    prepare: Option<String>,
    /// Command run on the machine, in the Box's copy, once a prepared Box is no longer open.
    #[arg(long, value_name = "CMD")]
    release: Option<String>,
    /// Command run on the machine, in the Box's copy, when a prepared Box falls asleep, with
    /// the Box's facts as env; stop what `prepare` started that holds the slot, such as its
    /// simulator.
    #[arg(long, value_name = "CMD")]
    suspend: Option<String>,
    /// Free GB a machine must have before a Box is prepared or synced there; below it the files
    /// of sleeping Boxes are removed, oldest first, and the call fails if that is not enough.
    /// 0 turns the check off.
    #[arg(long, default_value_t = 20, value_name = "GB")]
    min_free_gb: u64,
    /// Seconds prepare, suspend or release may run.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
    hook_timeout_secs: u64,
    /// Seconds between looks for prepared Boxes that are no longer open.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    sweep_secs: u64,
    /// Seconds between looks through each machine for files of Boxes the runner has no record of.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    reconcile_secs: u64,
    /// A stdio MCP server on the machine whose tools the runner lists as NAME_<tool>, as
    /// NAME=CMD; repeat for more.
    #[arg(long = "relay", value_name = "NAME=CMD", value_parser = relay)]
    relays: Vec<(String, String)>,
    /// An argument of a relay's tools set from the Box's fact on every call and hidden from the
    /// Box, as NAME.ARG=FACT; repeat for more.
    #[arg(long = "force-arg", value_name = "NAME.ARG=FACT", value_parser = force_arg)]
    forced: Vec<(String, String, String)>,
    /// A relay's tool left out of the listing and refused when called, as NAME.TOOL; repeat for
    /// more.
    #[arg(long = "hide-tool", value_name = "NAME.TOOL", value_parser = hide_tool)]
    hidden: Vec<(String, String)>,
    /// Seconds a relay session may go without a call before it ends.
    #[arg(long, default_value_t = 1800, value_parser = clap::value_parser!(u64).range(1..))]
    relay_idle_secs: u64,
    /// Seconds between pings on an open call's stream.
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..))]
    keep_alive_secs: u64,
}

/// Parses `NAME=CMD`, with NAME of `[a-z][a-z0-9-]*`.
fn relay(s: &str) -> Result<(String, String), String> {
    let (name, command) = s.split_once('=').ok_or("expected NAME=CMD")?;
    let mut chars = name.chars();
    let good = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !good {
        return Err(format!("NAME must be [a-z][a-z0-9-]*, not {name:?}"));
    }
    if command.trim().is_empty() {
        return Err("CMD is empty".to_owned());
    }
    Ok((name.to_owned(), command.to_owned()))
}

/// Parses `NAME.ARG=FACT`.
fn force_arg(s: &str) -> Result<(String, String, String), String> {
    let (target, fact) = s.split_once('=').ok_or("expected NAME.ARG=FACT")?;
    let (name, arg) = target.split_once('.').ok_or("expected NAME.ARG=FACT")?;
    if [name, arg, fact].iter().any(|p| p.is_empty()) {
        return Err("NAME, ARG and FACT must not be empty".to_owned());
    }
    Ok((name.to_owned(), arg.to_owned(), fact.to_owned()))
}

/// Parses `NAME.TOOL`.
fn hide_tool(s: &str) -> Result<(String, String), String> {
    let (name, tool) = s.split_once('.').ok_or("expected NAME.TOOL")?;
    if name.is_empty() || tool.is_empty() {
        return Err("NAME and TOOL must not be empty".to_owned());
    }
    Ok((name.to_owned(), tool.to_owned()))
}

/// The relays with their forced arguments; an error naming a relay given twice or a forced
/// argument or hidden tool of no relay.
fn relays(args: &Args) -> Result<Vec<Relay>, String> {
    let mut relays: Vec<Relay> = Vec::new();
    for (name, command) in &args.relays {
        if relays.iter().any(|r| &r.name == name) {
            return Err(format!("--relay {name} is given twice"));
        }
        relays.push(Relay {
            name: name.clone(),
            command: command.clone(),
            forced: std::collections::BTreeMap::new(),
            hidden: std::collections::BTreeSet::new(),
        });
    }
    for (name, arg, fact) in &args.forced {
        let relay = relays
            .iter_mut()
            .find(|r| &r.name == name)
            .ok_or_else(|| format!("--force-arg {name}.{arg} names no --relay {name}"))?;
        relay.forced.insert(arg.clone(), fact.clone());
    }
    for (name, tool) in &args.hidden {
        let relay = relays
            .iter_mut()
            .find(|r| &r.name == name)
            .ok_or_else(|| format!("--hide-tool {name}.{tool} names no --relay {name}"))?;
        relay.hidden.insert(tool.clone());
    }
    Ok(relays)
}

/// Parses `NAME=CAP`, with CAP at least 1.
fn resource(s: &str) -> Result<(String, usize), String> {
    let (name, cap) = s.split_once('=').ok_or("expected NAME=CAP")?;
    let cap = cap
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("CAP must be a whole number from 1, not {cap}"))?;
    if name.is_empty() {
        return Err("NAME is empty".to_owned());
    }
    Ok((name.to_owned(), cap))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let relays = match relays(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("moor-machine-runner: {e}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match tokio::net::TcpListener::bind(args.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("moor-machine-runner: cannot listen on {}: {e}", args.listen);
            return ExitCode::FAILURE;
        }
    };
    if let Ok(addr) = listener.local_addr() {
        eprintln!("moor-machine-runner listening on {addr}");
    }
    if let Some(addr) = args.agents {
        let agents = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("moor-machine-runner: cannot listen for agents on {addr}: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Ok(addr) = agents.local_addr() {
            eprintln!("moor-machine-runner taking agent links on {addr}");
        }
        let linked = Agents::new(&args.socket);
        tokio::spawn(async move {
            if let Err(e) = serve_agents(agents, linked).await {
                eprintln!("moor-machine-runner: agent links stopped: {e}");
            }
        });
    }
    let config = Config {
        socket: args.socket,
        state_dir: args.state_dir,
        service_host: args.service_host,
        resources: args.resources.into_iter().collect(),
        prepare: args.prepare,
        release: args.release,
        suspend: args.suspend,
        min_free_gb: args.min_free_gb,
        hook_timeout: Duration::from_secs(args.hook_timeout_secs),
        sweep: Duration::from_secs(args.sweep_secs),
        reconcile: Duration::from_secs(args.reconcile_secs),
        relays,
        relay_idle: Duration::from_secs(args.relay_idle_secs),
        keep_alive: Duration::from_secs(args.keep_alive_secs),
    };
    match serve(listener, config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("moor-machine-runner: {e}");
            ExitCode::FAILURE
        }
    }
}
