//! `moor controller serve`: a long-running process answering moor requests on a Unix socket.

pub mod attach;
mod conn;
mod drain;
mod front_proxy;
pub mod front_shim;
mod fronts;
mod hangup;
mod inherit;
mod local;
mod peer;
mod progress;
mod settle;
mod watch;

pub use local::watch_local;

use std::ffi::OsString;
use std::fs::{self, File, TryLockError};
use std::io::{BufReader, Write};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::parse_controller;
use moor_core::ErrorKind;

use drain::{Admitted, Drain, Handler};
use inherit::Sockets;

use crate::env::{Env, CONTROLLER_LOCK};
use crate::vault::{Vault, VAULT_KEY};
use crate::Moor;

/// Shared destination for diagnostics.
type Sink = Arc<Mutex<dyn Write + Send>>;

/// Exit code for an unusable config file.
const USAGE: u8 = 2;

/// Exit code for a state dir at another schema version, as for a refused CLI command.
const REFUSED: u8 = 3;

/// Serves the controller until SIGTERM, then drains; returns the process exit code.
///
/// The drain stops accepting and starts no settle pass, gives in-flight requests and a running
/// settle pass up to `drain_timeout_secs` to finish, answers waits, watches and requests it
/// did not run with a `restarting` frame, and exits 0. Each front gets SIGTERM when the drain
/// begins and SIGKILL when the budget ends; the controller exits once it has reaped each front's outer shim. Setup failures and the
/// "already running" notice go to `stderr`.
pub fn serve(env: &Env, config: &Path, stderr: impl Write + Send + 'static) -> u8 {
    let sink: Sink = Arc::new(Mutex::new(stderr));
    let mut running = match start(env, config) {
        Ok(running) => running,
        Err(stop) => {
            log(&sink, &stop.message);
            return stop.code;
        }
    };
    let wake = match drain::on_sigterm() {
        Ok(wake) => wake,
        Err(e) => {
            log(&sink, &format!("cannot handle SIGTERM: {e}"));
            return 1;
        }
    };
    log(&sink, &format!("listening on {}", running.socket.display()));
    for line in Moor::reconcile_proxies(&running.env) {
        log(&sink, &format!("proxy: {line}"));
    }
    let shim = std::env::current_exe().map_or_else(|_| "/proc/self/exe".into(), OsString::from);
    let mut fronts = running.supervise_fronts((shim, vec!["front-shim".into()]), &sink);
    let drain = Arc::new(Drain::default());
    drain.end_fronts_with_drain(fronts.terminator(running.drain_timeout));
    settle::spawn(&running.env, running.settle_every, &drain, &sink);
    running.serve_until(&wake, &drain, &sink);
    fronts.wait();
    0
}

/// Why the controller did not start.
#[derive(Debug)]
struct Stop {
    /// Process exit code.
    code: u8,
    /// Line for stderr, without the `moor controller` prefix.
    message: String,
}

/// A bound controller that has not begun accepting.
struct Running {
    /// Bound socket path.
    socket: PathBuf,
    /// Listening socket.
    listener: UnixListener,
    /// Each front's socket, by namespace.
    fronts: Vec<(String, UnixListener)>,
    /// The fronts that have a command, not yet started.
    commands: Vec<fronts::Front>,
    /// The front Proxy, not yet started; none when no front lists proxy rules.
    proxy: Option<front_proxy::Plan>,
    /// The vault key, for the user requests.
    vault: Arc<Vault>,
    /// Holds the single-instance flock until the process exits.
    _lock: File,
    /// Holds the one-controller-per-state-dir flock until the process exits.
    _state_lock: Option<File>,
    /// Environment shared by every connection.
    env: Arc<Env>,
    /// Time between settle passes.
    settle_every: Duration,
    /// How long a drain waits for in-flight requests.
    drain_timeout: Duration,
}

/// Loads `config`, takes the instance lock and the sockets passed in, binding any not passed.
fn start(env: &Env, config: &Path) -> Result<Running, Stop> {
    let refused = |message: String| Stop { code: 1, message };
    let mut sockets = Sockets::from_env(env).map_err(refused)?;
    let failed = |message: String| Stop {
        code: USAGE,
        message,
    };
    let text = fs::read_to_string(config)
        .map_err(|e| failed(format!("cannot read config {}: {e}", config.display())))?;
    let cfg = parse_controller(&config.display().to_string(), &text)
        .map_err(|e| failed(e.to_string()))?;
    let env = cfg.state_dir.as_ref().map_or_else(
        || env.clone(),
        |dir| env.clone().with_state_dir(&env.expand_home(Path::new(dir))),
    );
    let socket = match &cfg.socket {
        Some(s) => env.expand_home(Path::new(s)),
        None => state_socket(&env)
            .ok_or_else(|| failed("no state directory: set state_dir or MOOR_STATE_DIR".into()))?,
    };
    let env = Moor::open_store(env).map_err(|e| Stop {
        code: match e.kind {
            ErrorKind::Refused => REFUSED,
            ErrorKind::Usage => USAGE,
            _ => 1,
        },
        message: e.error.to_string(),
    })?;
    let (vault, key) = open_vault(&env, config)?;
    let (env, proxy) = front_proxy::plan(env.with_vault_key(key), &cfg.fronts).map_err(refused)?;
    let state_lock = env
        .state_dir()
        .map(|dir| take_lock(&dir.join(CONTROLLER_LOCK), &socket))
        .transpose()?;
    let lock = take_socket_lock(&socket)?;
    Moor::end_front_leases(&env).map_err(|e| refused(e.error.to_string()))?;
    let listener = sockets.listen("controller", &socket).map_err(refused)?;
    let fronts = peer::bind_fronts(&env, &mut sockets, &cfg.fronts.keys().collect::<Vec<_>>())?;
    let commands = fronts
        .iter()
        .filter_map(|(ns, listener)| {
            let access = proxy.as_ref().and_then(|p| p.access(ns));
            fronts::Front::new(ns, listener, cfg.fronts.get(ns)?, &env, &socket, access)
        })
        .collect::<Result<_, _>>()
        .map_err(refused)?;
    sockets.finish().map_err(refused)?;
    Ok(Running {
        socket,
        listener,
        fronts,
        commands,
        proxy,
        vault: Arc::new(vault),
        _lock: lock,
        _state_lock: state_lock,
        env: Arc::new(env),
        settle_every: cfg.settle_every(),
        drain_timeout: cfg.drain_timeout(),
    })
}

/// Loads the vault key beside `config`, creating it on first start, and returns it with its
/// path. A key inside the state dir is refused, so a copy of the state dir never carries the
/// key that opens it.
fn open_vault(env: &Env, config: &Path) -> Result<(Vault, PathBuf), Stop> {
    let refused = |message: String| Stop {
        code: REFUSED,
        message,
    };
    let dir = config.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = fs::canonicalize(dir.unwrap_or_else(|| Path::new(".")))
        .map_err(|e| refused(format!("cannot resolve the config dir: {e}")))?;
    let key = dir.join(VAULT_KEY);
    let state = env.state_dir().and_then(|s| fs::canonicalize(s).ok());
    if state.is_some_and(|s| dir.starts_with(s)) {
        return Err(refused(format!(
            "vault key {} is inside the state dir; keep the controller config outside it",
            key.display()
        )));
    }
    let vault = Vault::load_or_create(&key).map_err(|e| refused(e.to_string()))?;
    Ok((vault, key))
}

/// The socket path used when no explicit one is configured.
pub(crate) fn state_socket(env: &Env) -> Option<PathBuf> {
    env.state_dir().map(|dir| dir.join("controller.sock"))
}

/// Takes the flock beside `socket`, `<socket>.lock`.
fn take_socket_lock(socket: &Path) -> Result<File, Stop> {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    take_lock(Path::new(&path), socket)
}

/// Takes the flock on `path` and records our pid and socket in it.
///
/// A holder on our own socket exits 0, since the controller asked for is
/// already serving; a holder on another socket is a refusal.
fn take_lock(path: &Path, socket: &Path) -> Result<File, Stop> {
    let failed = |e: std::io::Error| Stop {
        code: 1,
        message: format!("cannot lock {}: {e}", path.display()),
    };
    if let Some(dir) = path.parent() {
        mkdir(dir).map_err(failed)?;
    }
    let mut file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(path)
        .map_err(failed)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)
                .and_then(|()| writeln!(file, "{}\n{}", std::process::id(), socket.display()))
                .map_err(failed)?;
            Ok(file)
        }
        Err(TryLockError::WouldBlock) => {
            let held = fs::read_to_string(path).unwrap_or_default();
            let mut lines = held.lines();
            let pid = lines.next().unwrap_or("?");
            let on = lines
                .next()
                .map_or_else(|| socket.display().to_string(), str::to_owned);
            Err(Stop {
                code: u8::from(Path::new(&on) != socket),
                message: format!("already running (pid {pid}) on {on}"),
            })
        }
        Err(TryLockError::Error(e)) => Err(failed(e)),
    }
}

/// Creates `dir` and missing parents, owner-only.
fn mkdir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Replaces any stale socket file with a fresh owner-only listener.
fn bind(socket: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        mkdir(dir)?;
    }
    // The lock proves no live controller owns this path.
    match fs::remove_file(socket) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

impl Running {
    /// Starts the fronts that have a command under the shim `program`; dropping the result
    /// stops them.
    fn supervise_fronts(
        &mut self,
        program: (OsString, Vec<OsString>),
        sink: &Sink,
    ) -> fronts::Supervised {
        let proxy = self.proxy.take().map(|plan| (program.0.clone(), plan));
        let shim = fronts::Shim::new(program);
        fronts::supervise(std::mem::take(&mut self.commands), shim, proxy, sink)
    }

    /// Serves the controller socket and each front socket until `wake` turns readable, then
    /// drains `drain`; the locks are held until it returns.
    fn serve_until(self, wake: &UnixStream, drain: &Arc<Drain>, sink: &Sink) {
        let mut listeners = vec![(self.listener, handler(&self.env, &self.vault, None, sink))];
        for (ns, listener) in self.fronts {
            let handler = handler(&self.env, &self.vault, Some(ns), sink);
            listeners.push((listener, handler));
        }
        match drain::serve_until(&listeners, wake, drain, self.drain_timeout, sink) {
            0 => log(sink, "drained; exiting"),
            left => log(
                sink,
                &format!(
                    "drain timed out; exiting with {left} connections or settle passes in flight"
                ),
            ),
        }
    }
}

#[cfg(test)]
impl Running {
    /// Serves on a thread that never drains.
    fn serve_in_background(self, sink: Sink) {
        std::thread::spawn(move || {
            let (wake, _never) = UnixStream::pair().expect("wake pair");
            self.serve_until(&wake, &Arc::new(Drain::default()), &sink);
        });
    }

    /// Serves on a thread until a byte is written to the returned socket, then drains; the
    /// thread ends when the controller would exit.
    fn serve_until_signalled(self, sink: Sink) -> (UnixStream, std::thread::JoinHandle<()>) {
        let (wake, signal) = UnixStream::pair().expect("wake pair");
        let serving =
            std::thread::spawn(move || self.serve_until(&wake, &Arc::new(Drain::default()), &sink));
        (signal, serving)
    }
}

/// Serves one connection on a listener; `front` names the front whose socket it is, none for
/// the controller socket.
fn handler(env: &Arc<Env>, vault: &Arc<Vault>, front: Option<String>, sink: &Sink) -> Handler {
    let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
    Arc::new(move |stream: UnixStream, conn: Admitted| {
        let peer = conn::Peer {
            caller: front.clone().map_or_else(
                || peer::local_caller(&stream, rustix::process::geteuid().as_raw()),
                |ns| Ok(moor_core::user::Caller::Front(ns)),
            ),
            vault: Arc::clone(&vault),
            hangup: hangup::Hangup::of(&stream).ok(),
            conn,
        };
        match stream.try_clone() {
            Ok(read) => conn::handle(BufReader::new(read), stream, &env, &peer),
            Err(e) => log(&sink, &format!("connection setup failed: {e}")),
        }
    })
}

/// Writes one `moor controller: <msg>` line; a failed write has nowhere to go.
fn log(sink: &Sink, msg: &str) {
    let _ = writeln!(
        sink.lock().unwrap_or_else(PoisonError::into_inner),
        "moor controller: {msg}"
    );
}

#[cfg(test)]
mod tests;
