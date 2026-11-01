//! The `moor-notify` binary.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Posts each configured user's Box events to their webhook.
#[derive(Parser)]
#[command(name = "moor-notify", version)]
struct Args {
    /// The controller's `notify` front socket; defaults to `$MOOR_FRONT_SOCKET`, which the
    /// controller sets when it runs the front.
    #[arg(long, env = "MOOR_FRONT_SOCKET")]
    socket: PathBuf,
    /// The hooks file.
    #[arg(long, env = "MOOR_NOTIFY_CONFIG")]
    config: PathBuf,
    /// Where each hook's cursor is kept; defaults to `$MOOR_FRONT_STATE_DIR`. Without one,
    /// each start posts only events newer than itself.
    #[arg(long, env = "MOOR_FRONT_STATE_DIR")]
    state_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let log = moor_notify::stderr_log();
    match moor_notify::serve(&args.socket, &args.config, args.state_dir, &log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            log(&why);
            ExitCode::FAILURE
        }
    }
}
