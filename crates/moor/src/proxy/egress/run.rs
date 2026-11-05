//! `moor proxy serve-egress`: the shared egress Proxy process.
//!
//! It takes `--state-dir DIR --listen ADDR` and optionally `--grace SECS`, `--rate N` and
//! `--burst N`, and the ledger key in `MOOR_EGRESS_KEY`. It reaches the controller through
//! the ledger socket in the state dir, and keeps each Box's CA and request log under
//! `<state-dir>/egress`. On SIGTERM or SIGINT it drains, then exits.

use std::ffi::OsString;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Clock;
use signal_hook::consts::{SIGINT, SIGTERM};
use zeroize::Zeroizing;

use super::{Config, Server};
use crate::adapters::clock::SystemClock;
use crate::env::{Env, EGRESS_KEY};
use crate::ledger_dir;

/// How long open tunnels get to finish after SIGTERM, unless `--grace` says otherwise.
const GRACE: Duration = Duration::from_secs(300);
/// Pause between audit deliveries and closed-Box sweeps.
const TEND: Duration = Duration::from_secs(2);
/// Pause between looks at the stop flag.
const POLL: Duration = Duration::from_millis(100);

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub(crate) struct Args {
    /// The controller's state dir.
    pub(crate) state_dir: Option<PathBuf>,
    /// Where to listen.
    pub(crate) listen: Option<SocketAddr>,
    /// How long open tunnels get after SIGTERM.
    pub(crate) grace: Duration,
    /// Tunnels a token may open per second.
    pub(crate) rate: f64,
    /// Tunnels a token may open at once.
    pub(crate) burst: f64,
}

impl Args {
    /// Parses the words after `serve-egress`.
    pub(crate) fn parse(words: &[OsString]) -> Result<Self, String> {
        let mut args = Self {
            state_dir: None,
            listen: None,
            grace: GRACE,
            rate: 20.0,
            burst: 100.0,
        };
        let mut words = words.iter().map(|w| w.to_string_lossy().into_owned());
        while let Some(flag) = words.next() {
            let value = words
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?;
            let bad = || format!("{flag}: bad value {value}");
            match flag.as_str() {
                "--state-dir" => args.state_dir = Some(value.into()),
                "--listen" => args.listen = Some(value.parse().map_err(|_| bad())?),
                "--grace" => args.grace = Duration::from_secs(value.parse().map_err(|_| bad())?),
                "--rate" => args.rate = value.parse().map_err(|_| bad())?,
                "--burst" => args.burst = value.parse().map_err(|_| bad())?,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(args)
    }
}

/// Runs the shared egress Proxy for `words`; returns the exit code.
///
/// # Errors
/// Why the Proxy could not start.
pub(crate) fn main(words: &[OsString]) -> Result<u8, String> {
    let args = Args::parse(words)?;
    let state_dir = args.state_dir.ok_or("--state-dir is required")?;
    let listen = args.listen.ok_or("--listen is required")?;
    let env = Env::capture();
    let key = env
        .get(EGRESS_KEY)
        .filter(|k| !k.is_empty())
        .map(|k| Zeroizing::new(k.to_owned()))
        .ok_or("MOOR_EGRESS_KEY is required")?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::from_env(&env).map_err(|e| e.to_string())?);
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    let server = Server::start(Config {
        listen,
        ledger: ledger_dir::socket(&state_dir),
        key,
        work_dir: state_dir.join("egress"),
        rate: args.rate,
        burst: args.burst,
        upstream_ca: test_upstream::ca(&env),
        guard_exempt: test_upstream::exempt(&env),
        clock: Arc::clone(&clock),
    })?;
    say(format_args!("listening on {}", server.addr()));
    let mut since = clock.instant();
    while !stop.load(Ordering::Acquire) {
        clock.sleep(POLL);
        if clock.instant().saturating_duration_since(since) >= TEND {
            server.tend();
            since = clock.instant();
        }
    }
    say(format_args!("draining for up to {}s", args.grace.as_secs()));
    let left = server.drain(args.grace);
    say(format_args!("drained; {left} tunnels were still open"));
    Ok(0)
}

/// Writes one `moor egress: ` line to stderr.
fn say(args: std::fmt::Arguments<'_>) {
    drop(writeln!(std::io::stderr(), "moor egress: {args}"));
}

/// The one local TLS upstream a live local run lets the Box proxies reach. Only the
/// `test-upstream` feature reads it.
mod test_upstream {
    use std::net::SocketAddr;
    use std::path::PathBuf;

    use crate::env::Env;

    /// The extra upstream CA PEM in `MOOR_TEST_UPSTREAM_CA`.
    #[cfg(feature = "test-upstream")]
    pub(super) fn ca(env: &Env) -> Option<PathBuf> {
        env.get("MOOR_TEST_UPSTREAM_CA").map(PathBuf::from)
    }

    /// The loopback address in `MOOR_TEST_UPSTREAM` the egress guard lets through.
    #[cfg(feature = "test-upstream")]
    pub(super) fn exempt(env: &Env) -> Vec<SocketAddr> {
        env.get("MOOR_TEST_UPSTREAM")
            .and_then(|a| a.parse::<SocketAddr>().ok())
            .filter(|a| a.ip().is_loopback())
            .into_iter()
            .collect()
    }

    /// No extra CA outside the test feature.
    #[cfg(not(feature = "test-upstream"))]
    pub(super) const fn ca(_: &Env) -> Option<PathBuf> {
        None
    }

    /// No exemption outside the test feature.
    #[cfg(not(feature = "test-upstream"))]
    pub(super) const fn exempt(_: &Env) -> Vec<SocketAddr> {
        Vec::new()
    }
}
