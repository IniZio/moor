//! `moor listen`: prints a line when a Box on a moor server needs attention, signed in with the
//! sign-in `moor login` saved.

use std::ffi::OsString;
use std::io;
use std::time::Duration;

use clap::Parser;
use moor_client::session::{normalize, Store};
use moor_listen::{Listen, Session};

use crate::cli::Output;

/// The arguments after `moor listen`.
#[derive(Debug, Parser)]
#[command(name = "moor listen")]
struct Args {
    /// The server, as its origin or its /mcp address.
    #[arg(long, env = "MOOR_SERVER")]
    server: String,
    /// Silence on the stream, in milliseconds, after which it counts as dead.
    #[arg(long, hide = true, default_value_t = 45_000)]
    idle_ms: u64,
    /// Milliseconds between looks for a sign-in while there is none.
    #[arg(long, hide = true, default_value_t = 60_000)]
    signed_out_ms: u64,
}

/// Runs `moor listen`, saying each line through `say`; `None` when `args` names another command.
#[must_use]
pub fn run(args: &[OsString], say: &mut dyn FnMut(&str) -> io::Result<()>) -> Option<Output> {
    if args.get(1)?.to_str()? != "listen" {
        return None;
    }
    let words = std::iter::once(OsString::from("moor listen")).chain(args.iter().skip(2).cloned());
    let parsed = match Args::try_parse_from(words) {
        Ok(parsed) => parsed,
        Err(e) if e.use_stderr() => return Some(done(String::new(), e.render().to_string(), 2)),
        Err(e) => return Some(done(e.render().to_string(), String::new(), 0)),
    };
    let origin = normalize(&parsed.server);
    let listen = Listen {
        url: format!("{origin}/mcp"),
        bearer: Box::new(Session::new(Store::user(), &origin)),
        idle: Duration::from_millis(parsed.idle_ms),
        signed_out: Duration::from_millis(parsed.signed_out_ms),
    };
    // The loop only ends when nobody reads the lines any more.
    Some(match listen.run(say) {
        Ok(()) => done(String::new(), String::new(), 0),
        Err(why) => done(String::new(), format!("moor-listen: {why}\n"), 1),
    })
}

/// An [`Output`].
const fn done(stdout: String, stderr: String, code: u8) -> Output {
    Output {
        stdout,
        stderr,
        code,
    }
}
