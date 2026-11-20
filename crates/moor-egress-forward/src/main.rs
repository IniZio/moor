//! `moor-egress-forward --config FILE`: the in-Box egress forwarder.
//! `moor-egress-forward --relay IP:PORT --socket PATH`: the host relay for rootless Docker.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use moor_egress_forward::{relay, say, serve, Config};

/// The command line's two modes.
const USAGE: &str = "usage: moor-egress-forward --config FILE | --relay IP:PORT --socket PATH";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            say(format_args!("{why}"));
            ExitCode::FAILURE
        }
    }
}

/// What the command line asks for.
enum Mode {
    /// The in-Box forwarder.
    Forward(Config),
    /// The relay to a Unix socket.
    Relay(PathBuf),
}

/// The mode and the address it listens on, from the command line.
fn parse(args: &[String]) -> Result<(SocketAddr, Mode), String> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--config", path] => {
            let json =
                std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            let config = Config::parse(&json)?;
            Ok((config.listen, Mode::Forward(config)))
        }
        ["--relay", addr, "--socket", socket] => {
            let listen = addr
                .parse()
                .map_err(|_| format!("{addr} is not an IP:PORT address"))?;
            Ok((listen, Mode::Relay(PathBuf::from(socket))))
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Reads the command line and serves until the listener fails.
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (listen, mode) = parse(&args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot build the runtime: {e}"))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
        say(format_args!("listening on {listen}"));
        match mode {
            Mode::Forward(config) => serve(listener, Arc::new(config.upstream)).await,
            Mode::Relay(socket) => relay(listener, socket).await,
        }
        Ok(())
    })
}
