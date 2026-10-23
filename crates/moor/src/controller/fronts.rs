//! Runs each front that has a command as a child of the controller, restarting it when it exits.
//! Each runs under `moor front-shim`, which keeps the controller's own files out of its reach.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead as _, BufReader, Read};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use command_fds::{CommandFdExt as _, FdMapping};
use rustix::fs::{Mode, OFlags};

use super::{log, Sink};

/// The fd a front finds its own socket on.
const SOCKET_FD: i32 = 3;

/// Wait before restarting a front that exited; it doubles while the front keeps exiting soon
/// after it starts.
const FIRST_BACKOFF: Duration = Duration::from_millis(500);

/// Longest wait before a restart.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A front that ran this long before it exited is restarted after [`FIRST_BACKOFF`].
const STEADY: Duration = Duration::from_secs(60);

/// How often a supervisor checks whether its front exited.
const POLL: Duration = Duration::from_millis(50);

/// A front the controller runs.
pub(super) struct Front {
    /// The front's namespace.
    ns: String,
    /// Its socket; the child gets an `O_PATH` fd to it.
    socket: PathBuf,
    /// Its argv, not empty.
    command: Vec<String>,
}

impl Front {
    /// The front `ns` listening on `listener`, run as `command`.
    pub(super) fn new(
        ns: &str,
        listener: &UnixListener,
        command: Vec<String>,
    ) -> Result<Self, String> {
        let socket = listener
            .local_addr()
            .ok()
            .and_then(|a| a.as_pathname().map(PathBuf::from))
            .ok_or_else(|| format!("front {ns}: its socket has no path"))?;
        Ok(Self {
            ns: ns.to_owned(),
            socket,
            command,
        })
    }

    /// `<state>/fronts/<ns>.pid`: the running front's pid and its start time from `/proc`.
    fn pid_file(&self) -> PathBuf {
        self.socket.with_extension("pid")
    }
}

/// Runs each front under `moor front-shim`, hiding what a front must not reach.
pub(super) struct Shim {
    /// The program that runs `moor front-shim`, and its first arguments.
    program: (OsString, Vec<OsString>),
    /// `--cover` the state dir, `--mask` the operator socket and the vault key.
    hide: Vec<OsString>,
}

impl Shim {
    /// Runs `program` as the shim, hiding `state_dir`, `socket` and `vault_key` from each front.
    pub(super) fn new(
        program: (OsString, Vec<OsString>),
        state_dir: Option<&Path>,
        socket: &Path,
        vault_key: Option<&Path>,
    ) -> Self {
        let cover = state_dir.into_iter().map(|dir| ("--cover", dir));
        let mask = [Some(socket), vault_key]
            .into_iter()
            .flatten()
            .map(|file| ("--mask", file));
        let hide = cover
            .chain(mask)
            .flat_map(|(flag, path)| [OsString::from(flag), path.as_os_str().to_owned()])
            .collect();
        Self { program, hide }
    }

    /// The command that runs the shim, without its own arguments.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program.0);
        command.args(&self.program.1);
        command
    }

    /// Whether this host lets the shim create the namespaces each front runs in.
    fn check(&self) -> Result<(), String> {
        let out = self
            .command()
            .arg("--check")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// The running supervisors; dropping it kills each front and waits for its supervisor to end.
pub(super) struct Supervised {
    /// Dropping a sender tells its supervisor to stop.
    stops: Vec<Sender<()>>,
    /// One supervisor per front.
    threads: Vec<JoinHandle<()>>,
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.stops.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Starts one supervisor thread per front, unless the host forbids the namespaces each front
/// runs in; then it starts none and logs which sysctls allow them.
pub(super) fn supervise(mut fronts: Vec<Front>, shim: Shim, sink: &Sink) -> Supervised {
    let shim = Arc::new(shim);
    if let Some(Err(e)) = (!fronts.is_empty()).then(|| shim.check()) {
        log(
            sink,
            &format!(
                "fronts not started: they need unprivileged user namespaces \
                 (sysctl kernel.unprivileged_userns_clone=1, user.max_user_namespaces above 0, \
                 kernel.apparmor_restrict_unprivileged_userns=0): {e}"
            ),
        );
        fronts.clear();
    }
    let (stops, threads) = fronts
        .into_iter()
        .map(|front| {
            let (stop, stopped) = mpsc::channel();
            let (sink, shim) = (sink.clone(), Arc::clone(&shim));
            (
                stop,
                thread::spawn(move || keep_running(&front, &shim, &stopped, &sink)),
            )
        })
        .unzip();
    Supervised { stops, threads }
}

/// Runs `front` until `stop` disconnects, restarting it with backoff whenever it exits.
#[expect(
    clippy::disallowed_methods,
    reason = "times how long a child ran to pick its restart backoff, not domain time"
)]
fn keep_running(front: &Front, shim: &Shim, stop: &Receiver<()>, sink: &Sink) {
    let ns = &front.ns;
    let mut backoff = FIRST_BACKOFF;
    loop {
        let started = Instant::now();
        let exited = match spawn(front, shim, sink) {
            Ok(mut child) => {
                let status = watch(&mut child, stop);
                let _ = fs::remove_file(front.pid_file());
                let Some(status) = status else { return };
                format!("exited with {status}")
            }
            Err(e) => format!("cannot start: {e}"),
        };
        if started.elapsed() >= STEADY {
            backoff = FIRST_BACKOFF;
        }
        log(
            sink,
            &format!("front {ns}: {exited}; restarting in {backoff:?}"),
        );
        if !matches!(stop.recv_timeout(backoff), Err(RecvTimeoutError::Timeout)) {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Waits for `child` to exit; kills it and returns `None` once `stop` disconnects.
fn watch(child: &mut Child, stop: &Receiver<()>) -> Option<ExitStatus> {
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if !matches!(stop.recv_timeout(POLL), Err(RecvTimeoutError::Timeout)) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    }
}

/// Starts `front` under `shim` in `/`, with its socket on fd 3, relays its stdout and stderr
/// lines to `sink` and records the shim's pid. The shim exits with this controller.
fn spawn(front: &Front, shim: &Shim, sink: &Sink) -> io::Result<Child> {
    let ns = &front.ns;
    let socket = rustix::fs::open(&front.socket, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())?;
    let mut command = shim.command();
    command
        .args(["--parent", &process::id().to_string()])
        .args(&shim.hide)
        .arg("--")
        .args(&front.command)
        .env("MOOR_FRONT_SOCKET", format!("/proc/self/fd/{SOCKET_FD}"))
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .fd_mappings(vec![FdMapping {
            parent_fd: socket,
            child_fd: SOCKET_FD,
        }])
        .map_err(io::Error::other)?;
    let mut child = command.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        relay_lines(stdout, ns, sink);
    }
    if let Some(stderr) = child.stderr.take() {
        relay_lines(stderr, ns, sink);
    }
    let pid = child.id();
    match write_pid_file(front, pid) {
        Ok(()) => log(sink, &format!("front {ns}: started pid {pid}")),
        Err(e) => log(
            sink,
            &format!("front {ns}: started pid {pid}; no pid file: {e}"),
        ),
    }
    Ok(child)
}

/// Logs each line of `stream`, a front's output, to `sink` until the front closes it.
fn relay_lines(stream: impl Read + Send + 'static, ns: &str, sink: &Sink) {
    let (sink, ns) = (sink.clone(), ns.to_owned());
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            log(&sink, &format!("front {ns}: {line}"));
        }
    });
}

/// Replaces the pid file of `front` with `pid` and its start time.
fn write_pid_file(front: &Front, pid: u32) -> io::Result<()> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let started = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other(format!("no start time in /proc/{pid}/stat")))?;
    let path = front.pid_file();
    let partial = path.with_extension("pid.partial");
    fs::write(&partial, format!("{pid}\n{started}\n"))?;
    fs::rename(&partial, &path)
}
