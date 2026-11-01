//! The `moor-notify` binary.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use moor_client::Client;
use moor_notify::guard::Reach;
use moor_notify::work::{self, Options};

/// Posts the controller's queued webhook deliveries.
#[derive(Parser)]
#[command(name = "moor-notify", version)]
struct Args {
    /// The controller's `notify` front socket; defaults to `$MOOR_FRONT_SOCKET`, which the
    /// controller sets when it runs the front.
    #[arg(long, env = "MOOR_FRONT_SOCKET")]
    socket: PathBuf,
    /// How long to wait, in milliseconds, before claiming again after a claim found nothing.
    #[arg(long, default_value_t = 1000)]
    poll_ms: u64,
    /// Post to any address, loopback and private ones included, for local development.
    #[arg(long)]
    allow_private: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let log = moor_notify::stderr_log();
    let reach = if args.allow_private {
        log("warning: --allow-private is on, so webhooks may reach loopback and private addresses");
        Reach::Any
    } else {
        Reach::Public
    };
    let options = Options {
        poll: Duration::from_millis(args.poll_ms),
        reach,
        ..Options::default()
    };
    match work::run(&Client::new(&args.socket), &options, &log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            log(&why);
            ExitCode::FAILURE
        }
    }
}
