//! `moor controller serve`: a long-running process answering moor requests on a Unix socket.

pub mod attach;
mod conn;
mod progress;
mod settle;
mod watch;
mod web;
mod web_feed;

use std::fs::{self, File, TryLockError};
use std::io::{BufReader, Write};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::config::{parse_controller, WebConfig};

use crate::env::Env;
use crate::Moor;

/// Shared destination for diagnostics.
type Sink = Arc<Mutex<dyn Write + Send>>;

/// Exit code for an unusable config file.
const USAGE: u8 = 2;

/// Serves the controller until killed; returns the process exit code.
///
/// Setup failures and the "already running" notice go to `stderr`.
pub fn serve(env: &Env, config: &Path, stderr: impl Write + Send + 'static) -> u8 {
    let sink: Sink = Arc::new(Mutex::new(stderr));
    match start(env, config) {
        Ok(running) => {
            log(&sink, &format!("listening on {}", running.socket.display()));
            if let Some(web) = &running.web {
                log(&sink, &format!("web ui on http://{}", web.addr()));
            }
            for line in Moor::reconcile_proxies(&running.env) {
                log(&sink, &format!("proxy: {line}"));
            }
            settle::spawn(&running.env, running.settle_every, &sink);
            running.accept_loop(&sink);
            0
        }
        Err(stop) => {
            log(&sink, &stop.message);
            stop.code
        }
    }
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
    /// Holds the single-instance flock until the process exits.
    _lock: File,
    /// Loopback web UI, served until the process exits.
    web: Option<web::Web>,
    /// Environment shared by every connection.
    env: Arc<Env>,
    /// Time between settle passes.
    settle_every: Duration,
}

/// Loads `config`, takes the instance lock and binds the socket.
fn start(env: &Env, config: &Path) -> Result<Running, Stop> {
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
    let lock = take_lock(&socket)?;
    let listener = bind(&socket).map_err(|e| Stop {
        code: 1,
        message: format!("cannot bind {}: {e}", socket.display()),
    })?;
    let web = cfg.web.as_ref().map(|w| start_web(&env, w)).transpose()?;
    Ok(Running {
        socket,
        listener,
        _lock: lock,
        web,
        env: Arc::new(env),
        settle_every: cfg.settle_every(),
    })
}

/// Reads the token and binds the web UI on loopback.
fn start_web(env: &Env, web: &WebConfig) -> Result<web::Web, Stop> {
    let token_file = env.expand_home(&web.token_file);
    let token = web_feed::read_token(&token_file).map_err(|message| Stop {
        code: USAGE,
        message,
    })?;
    web::start(
        web.port,
        token,
        web_feed::factory(env),
        Duration::from_millis(200),
    )
    .map_err(|e| Stop {
        code: 1,
        message: format!("cannot bind web ui on 127.0.0.1:{}: {e}", web.port),
    })
}

/// The socket path used when no explicit one is configured.
pub(crate) fn state_socket(env: &Env) -> Option<PathBuf> {
    env.state_dir().map(|dir| dir.join("controller.sock"))
}

/// Takes `<socket>.lock` and records our pid and socket in it.
fn take_lock(socket: &Path) -> Result<File, Stop> {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    let path = PathBuf::from(path);
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
        .open(&path)
        .map_err(failed)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)
                .and_then(|()| writeln!(file, "{}\n{}", std::process::id(), socket.display()))
                .map_err(failed)?;
            Ok(file)
        }
        Err(TryLockError::WouldBlock) => {
            let held = fs::read_to_string(&path).unwrap_or_default();
            let mut lines = held.lines();
            let pid = lines.next().unwrap_or("?");
            let on = lines
                .next()
                .map_or_else(|| socket.display().to_string(), str::to_owned);
            Err(Stop {
                code: 0,
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
    /// Serves each connection on its own thread; never returns.
    fn accept_loop(&self, sink: &Sink) {
        for stream in self.listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    log(sink, &format!("accept failed: {e}"));
                    continue;
                }
            };
            let env = Arc::clone(&self.env);
            let sink = Arc::clone(sink);
            thread::spawn(move || match stream.try_clone() {
                Ok(read) => conn::handle(BufReader::new(read), stream, &env),
                Err(e) => log(&sink, &format!("connection setup failed: {e}")),
            });
        }
    }
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
