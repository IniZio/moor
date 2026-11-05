//! `moor-egress-forward --config FILE`: the in-Box egress forwarder.

use std::process::ExitCode;
use std::sync::Arc;

use moor_egress_forward::{say, serve, Config};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            say(format_args!("{why}"));
            ExitCode::FAILURE
        }
    }
}

/// Reads the config named on the command line and serves until the listener fails.
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [flag, path] = args.as_slice() else {
        return Err("usage: moor-egress-forward --config FILE".to_owned());
    };
    if flag != "--config" {
        return Err("usage: moor-egress-forward --config FILE".to_owned());
    }
    let json = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let config = Config::parse(&json)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot build the runtime: {e}"))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(config.listen)
            .await
            .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
        say(format_args!("listening on {}", config.listen));
        serve(listener, Arc::new(config.upstream)).await;
        Ok(())
    })
}
