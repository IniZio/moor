//! `moor-listen`: holds a `subscriptions/listen` stream open on the moor gate and prints one
//! line per Box event worth waking for. It knows nothing of whoever reads its stdout.

mod gate;
mod sse;
mod watch;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

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
}

/// Writes `line` to `out` and flushes, so a reader sees it at once.
fn emit(out: &mut impl Write, line: &str) -> io::Result<()> {
    writeln!(out, "{line}")?;
    out.flush()
}

fn main() -> ExitCode {
    let args = Args::parse();
    let gate = gate::Gate::new(args.url, args.token_cmd);
    let mut stdout = io::stdout().lock();
    // The loop only ends when stdout is gone.
    let Err(why) = watch::run(&gate, &mut |line| emit(&mut stdout, line)) else {
        return ExitCode::SUCCESS;
    };
    let _ = emit(&mut io::stderr(), &format!("moor-listen: {why}"));
    ExitCode::FAILURE
}
