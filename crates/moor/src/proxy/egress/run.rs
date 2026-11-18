//! `moor proxy serve-egress`: the shared egress Proxy process.
//!
//! It takes `--state-dir DIR --listen ADDR`, where an absolute path is a Unix socket, and
//! optionally `--also-listen IP:PORT`, a TCP address it serves as well as `--listen`, `--ledger ADDR`, `--ledger-ca PEM`, `--ledger-cert PEM`, `--ledger-key PEM`,
//! `--machine-service HOST=ADDR`, `--moor-service HOST=ADDR`, `--grace SECS`,
//! `--rate N`, `--burst N`, `--lookup-rate N`, `--tunnels N`, `--warm-every SECS` and
//! `--until-stdin-closes`, and the ledger key in
//! `MOOR_EGRESS_KEY`. It reaches the controller through the ledger socket in the state dir, or
//! through `--ledger`: a socket path, or `tls://host:port` checked against the CA bundle
//! `--ledger-ca`, presenting the client certificate `--ledger-cert` with its key `--ledger-key`. It keeps each Box's CA and request log under `<state-dir>/egress`. On SIGTERM
//! or SIGINT, once its Unix socket is removed or replaced, or, with `--until-stdin-closes`, once
//! its stdin closes, it drains, then exits. The controller that starts it holds the other end of
//! that stdin, so the Proxy stops with the controller however the controller ends.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::Served;
use moor_core::seam::clock::Clock;
use signal_hook::consts::{SIGINT, SIGTERM};
use zeroize::Zeroizing;

use super::{Config, Listen, Rate, ServedRoute, Server, Warm};
use crate::adapters::clock::SystemClock;
use crate::env::{Env, EGRESS_KEY, MACHINE_SERVICE_KEY, MOOR_SERVICE_KEY};
use crate::ledger_dir;
use crate::proxy::ledger::{Endpoint, TlsFiles};

/// How long open tunnels get to finish after SIGTERM, unless `--grace` says otherwise.
const GRACE: Duration = Duration::from_secs(300);
/// Pause between audit deliveries and closed-Box sweeps.
const TEND: Duration = Duration::from_secs(2);
/// A warming pass that takes longer leaves the replica not Ready.
const WARM_DEADLINE: Duration = Duration::from_secs(8);
/// Pause between looks at the stop flag.
const POLL: Duration = Duration::from_millis(100);

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub(crate) struct Args {
    /// The controller's state dir.
    pub(crate) state_dir: Option<PathBuf>,
    /// Where to listen.
    pub(crate) listen: Option<Listen>,
    /// Where the ledger is, when not the socket in the state dir.
    pub(crate) ledger: Option<String>,
    /// The CA bundle that checks a TLS ledger's certificate.
    pub(crate) ledger_ca: Option<PathBuf>,
    /// The client certificate a TLS ledger checks.
    pub(crate) ledger_cert: Option<PathBuf>,
    /// The private key of `ledger_cert`.
    pub(crate) ledger_key: Option<PathBuf>,
    /// A TCP address to also listen on, for the Boxes of Docker machines on this host.
    pub(crate) also: Option<std::net::SocketAddr>,
    /// The hosts it serves itself, each from its own flag.
    pub(crate) served: Vec<ServedRoute>,
    /// How long open tunnels get after SIGTERM.
    pub(crate) grace: Duration,
    /// Tunnels a token may open per second.
    pub(crate) rate: f64,
    /// Tunnels a token may open at once.
    pub(crate) burst: f64,
    /// Lookups a second, and at once, for tokens not yet confirmed, shared by all of them.
    pub(crate) lookup_rate: f64,
    /// Most tunnels one token may hold open at once.
    pub(crate) tunnels: usize,
    /// Pause between passes that ready every awake Box; none readies each on first use only.
    pub(crate) warm_every: Option<Duration>,
    /// Whether to stop once stdin closes.
    pub(crate) until_stdin_closes: bool,
}

impl Args {
    /// Parses the words after `serve-egress`.
    pub(crate) fn parse(words: &[OsString]) -> Result<Self, String> {
        let mut args = Self {
            state_dir: None,
            listen: None,
            also: None,
            ledger: None,
            ledger_ca: None,
            ledger_cert: None,
            ledger_key: None,
            served: Vec::new(),
            grace: GRACE,
            rate: 20.0,
            burst: 100.0,
            lookup_rate: 200.0,
            tunnels: 64,
            warm_every: None,
            until_stdin_closes: false,
        };
        let mut words = words.iter().map(|w| w.to_string_lossy().into_owned());
        while let Some(flag) = words.next() {
            if flag == "--until-stdin-closes" {
                args.until_stdin_closes = true;
                continue;
            }
            let value = words
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?;
            let bad = || format!("{flag}: bad value {value}");
            match flag.as_str() {
                "--state-dir" => args.state_dir = Some(value.into()),
                "--listen" if value.starts_with('/') => {
                    args.listen = Some(Listen::Unix(value.into()));
                }
                "--listen" => args.listen = Some(Listen::Tcp(value.parse().map_err(|_| bad())?)),
                "--also-listen" => args.also = Some(value.parse().map_err(|_| bad())?),
                "--ledger" => args.ledger = Some(value),
                "--ledger-ca" => args.ledger_ca = Some(value.into()),
                "--ledger-cert" => args.ledger_cert = Some(value.into()),
                "--ledger-key" => args.ledger_key = Some(value.into()),
                "--machine-service" => args.serve(Served::Machine, &value).ok_or_else(bad)?,
                "--moor-service" => args.serve(Served::Moor, &value).ok_or_else(bad)?,
                "--grace" => args.grace = Duration::from_secs(value.parse().map_err(|_| bad())?),
                "--rate" => args.rate = value.parse().map_err(|_| bad())?,
                "--burst" => args.burst = value.parse().map_err(|_| bad())?,
                "--lookup-rate" => args.lookup_rate = value.parse().map_err(|_| bad())?,
                "--tunnels" => args.tunnels = value.parse().map_err(|_| bad())?,
                "--warm-every" => {
                    let secs: u64 = value.parse().map_err(|_| bad())?;
                    args.warm_every = (secs > 0).then(|| Duration::from_secs(secs));
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(args)
    }

    /// Serves `HOST=ADDR` for `served`, in place of any earlier flag for it.
    fn serve(&mut self, served: Served, value: &str) -> Option<()> {
        let route = ServedRoute::parse(served, value)?;
        self.served.retain(|r| r.served != served);
        self.served.push(route);
        Some(())
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
    let ledger = args.ledger.unwrap_or_else(|| {
        ledger_dir::socket(&state_dir)
            .to_string_lossy()
            .into_owned()
    });
    let tls = match (&args.ledger_ca, &args.ledger_cert, &args.ledger_key) {
        (Some(ca), Some(cert), Some(key)) => Some(TlsFiles { ca, cert, key }),
        (None, None, None) => None,
        _ => return Err("--ledger-ca, --ledger-cert and --ledger-key go together".to_owned()),
    };
    let ledger = Endpoint::parse(&ledger, tls).map_err(|e| format!("--ledger: {e}"))?;
    let env = Env::capture();
    let routes = args
        .served
        .into_iter()
        .map(|r| keyed(r, &env))
        .collect::<Result<_, _>>()?;
    let key = env
        .get(EGRESS_KEY)
        .filter(|k| !k.is_empty())
        .map(|k| Zeroizing::new(k.to_owned()))
        .ok_or("MOOR_EGRESS_KEY is required")?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::from_env(&env).map_err(|e| e.to_string())?);
    let stop = stop_flag(args.until_stdin_closes)?;
    let server = Server::start(Config {
        listen,
        also: args.also,
        ledger,
        key,
        work_dir: state_dir.join("egress"),
        rate: Rate {
            per_sec: args.rate,
            burst: args.burst,
        },
        lookups: Rate {
            per_sec: args.lookup_rate,
            burst: args.lookup_rate,
        },
        tunnels: args.tunnels,
        tend: TEND,
        log: Arc::new(Mutex::new(std::io::stderr())),
        upstream_ca: test_upstream::ca(&env),
        guard_exempt: test_upstream::exempt(&env),
        served: routes,
        warm: args.warm_every.map(|every| Warm {
            every,
            deadline: WARM_DEADLINE,
        }),
        clock: Arc::clone(&clock),
    })?;
    say(format_args!("listening on {}", server.listening()));
    wait_for_stop(server.listening(), &stop, clock.as_ref());
    say(format_args!("draining for up to {}s", args.grace.as_secs()));
    let left = server.drain(args.grace);
    say(format_args!("drained; {left} tunnels were still open"));
    Ok(0)
}

/// A flag raised on SIGTERM or SIGINT and, when `until_stdin_closes`, once stdin reaches its
/// end or fails.
fn stop_flag(until_stdin_closes: bool) -> Result<Arc<AtomicBool>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    if until_stdin_closes {
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("moor-egress-stdin".to_owned())
            .spawn(move || {
                drop(std::io::copy(&mut std::io::stdin(), &mut std::io::sink()));
                say(format_args!("stdin closed"));
                stop.store(true, Ordering::Release);
            })
            .map_err(|e| format!("cannot watch stdin: {e}"))?;
    }
    Ok(stop)
}

/// `route` with the key its service is sent: `MOOR_MACHINE_SERVICE_KEY` for the machine service,
/// `MOOR_SERVICE_KEY` for moor's own. A machine key left empty is none; an empty moor key is
/// refused, so a mis-set secret never leaves the Box listener unguarded.
fn keyed(route: ServedRoute, env: &Env) -> Result<ServedRoute, String> {
    let (name, empty_ok) = match route.served {
        Served::Machine => (MACHINE_SERVICE_KEY, true),
        Served::Moor => (MOOR_SERVICE_KEY, false),
    };
    let key = env.get(name);
    if !empty_ok && key.is_some_and(str::is_empty) {
        return Err(format!("{name} is set but empty"));
    }
    let key = key.filter(|k| !k.is_empty());
    if key.is_some_and(|k| !k.bytes().all(|b| b.is_ascii_graphic())) {
        return Err(format!("{name} must be printable with no spaces"));
    }
    Ok(ServedRoute {
        key: key.map(Arc::from),
        ..route
    })
}

/// Returns once `stop` is set, or once the Unix socket the Proxy listens on is removed or
/// another Proxy's.
fn wait_for_stop(listening: &Listen, stop: &AtomicBool, clock: &dyn Clock) {
    let socket = match listening {
        Listen::Unix(path) => Some((path, inode(path))),
        Listen::Tcp(_) => None,
    };
    while !stop.load(Ordering::Acquire) {
        if socket.is_some_and(|(path, bound)| inode(path) != bound) {
            say(format_args!("its socket is gone or another Proxy's now"));
            return;
        }
        clock.sleep(POLL);
    }
}

/// The device and inode of the file at `path`; `None` when it is gone.
fn inode(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
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

#[cfg(test)]
mod tests;
