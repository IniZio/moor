//! `moor-machine-runner`: serve the machine runner MCP server on the host.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use moor_machine_runner::{serve, Config};

/// Serves the tools a Box uses to work on its attached machine.
#[derive(Parser)]
#[command(name = "moor-machine-runner")]
struct Args {
    /// Address to listen on; the Box's Proxy routes the service host here.
    #[arg(long, default_value = "127.0.0.1:7700")]
    listen: SocketAddr,
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
    let config = Config {
        socket: args.socket,
        state_dir: args.state_dir,
        service_host: args.service_host,
        resources: args.resources.into_iter().collect(),
    };
    match serve(listener, config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("moor-machine-runner: {e}");
            ExitCode::FAILURE
        }
    }
}
