//! `moor-listen`: the listener with a shell command for its token. `moor listen` is the same
//! listener signed in with `moor login`.

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use moor_listen::{Listen, Shell};

/// Prints a line when a Box needs attention.
#[derive(Parser)]
#[command(name = "moor-listen", version)]
struct Args {
    /// The gate's MCP endpoint, such as `https://host/mcp`.
    #[arg(long, env = "MOOR_MCP_URL")]
    url: String,
    /// Shell command whose stdout is the bearer token; it runs before every connect.
    #[arg(long, env = "MOOR_TOKEN_CMD")]
    token_cmd: String,
    /// Silence on the stream, in milliseconds, after which it counts as dead.
    #[arg(long, hide = true, default_value_t = 45_000)]
    idle_ms: u64,
    /// Watch only this Box; repeat for several. Without it every Box is watched.
    #[arg(long = "box", value_name = "NAME")]
    boxes: Vec<String>,
}

/// Writes `line` to `out` and flushes, so a reader sees it at once.
fn emit(out: &mut impl Write, line: &str) -> io::Result<()> {
    writeln!(out, "{line}")?;
    out.flush()
}

fn main() -> ExitCode {
    let args = Args::parse();
    let listen = Listen {
        url: args.url,
        bearer: Box::new(Shell(args.token_cmd)),
        idle: Duration::from_millis(args.idle_ms),
        signed_out: Listen::SIGNED_OUT,
        only: args.boxes,
    };
    let mut stdout = io::stdout().lock();
    // The loop ends when stdout is gone or every named Box has closed.
    let Err(why) = listen.run(&mut |line| emit(&mut stdout, line)) else {
        return ExitCode::SUCCESS;
    };
    let _ = emit(&mut io::stderr(), &format!("moor-listen: {why}"));
    ExitCode::FAILURE
}
