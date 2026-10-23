//! Runs each front that has a command as a child of the controller, restarting it when it exits.
//! Each runs under `moor front-shim`, which keeps the controller's own files out of its reach.

use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::process::{self, Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use command_fds::{CommandFdExt as _, FdMapping};
#[cfg(target_os = "linux")]
use rustix::fs::{Mode, OFlags};
use rustix::process::{kill_process, Pid, Signal};

use super::front_proxy::{self, Plan};
use super::{log, Sink};

mod front;
#[cfg(target_os = "linux")]
mod leftover;
mod relay;

pub(super) use front::Front;
pub(super) use relay::relay_lines;

/// The fd a front finds its own socket on.
#[cfg(target_os = "linux")]
const SOCKET_FD: i32 = 3;

/// Wait before restarting a front that exited; it doubles while the front keeps exiting soon
/// after it starts.
pub(super) const FIRST_BACKOFF: Duration = Duration::from_millis(500);

/// Longest wait before a restart.
pub(super) const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A front that ran this long before it exited is restarted after [`FIRST_BACKOFF`].
pub(super) const STEADY: Duration = Duration::from_secs(60);

/// How often a supervisor checks whether its front exited.
const POLL: Duration = Duration::from_millis(50);

/// Runs each front under `moor front-shim`, which gives it a root of its own.
pub(super) struct Shim {
    /// The program that runs `moor front-shim`, and its first arguments.
    program: (OsString, Vec<OsString>),
}

impl Shim {
    /// Runs `program` as the shim.
    pub(super) const fn new(program: (OsString, Vec<OsString>)) -> Self {
        Self { program }
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

/// What a supervisor is told to do with its front.
#[derive(Clone, Copy, Debug)]
pub(super) enum Stop {
    /// Send it SIGTERM, restart it no more, and kill it if it is still running after the grace.
    Term(Duration),
    /// Kill it now.
    Kill,
}

/// Tells every supervisor to stop its front, once the controller begins to drain.
#[derive(Clone, Debug)]
pub(super) struct Terminator {
    /// One per supervisor.
    stops: Vec<Sender<Stop>>,
    /// How long a front gets to exit after SIGTERM.
    grace: Duration,
}

impl Terminator {
    /// Sends SIGTERM to each front; each is killed if still running after the grace.
    pub(super) fn terminate(&self) {
        for stop in &self.stops {
            let _ = stop.send(Stop::Term(self.grace));
        }
    }
}

/// The running supervisors; dropping it kills each front and waits for its supervisor to end.
pub(super) struct Supervised {
    /// One per supervisor.
    stops: Vec<Sender<Stop>>,
    /// One supervisor per front.
    threads: Vec<JoinHandle<()>>,
}

impl Supervised {
    /// A [`Terminator`] that gives each front `grace` to exit.
    pub(super) fn terminator(&self, grace: Duration) -> Terminator {
        Terminator {
            stops: self.stops.clone(),
            grace,
        }
    }

    /// Waits until every supervisor has ended; with a [`Terminator`] fired, that is within its
    /// grace.
    pub(super) fn wait(&mut self) {
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        for stop in &self.stops {
            let _ = stop.send(Stop::Kill);
        }
        self.wait();
    }
}

/// Starts one supervisor thread per front, unless the host forbids the namespaces or the
/// sandbox each front runs in; then it starts none and logs why. With `proxy`, the moor binary
/// and the front Proxy it runs, one more thread keeps that Proxy running.
pub(super) fn supervise(
    mut fronts: Vec<Front>,
    shim: Shim,
    proxy: Option<(OsString, Plan)>,
    sink: &Sink,
) -> Supervised {
    let shim = Arc::new(shim);
    if let Some(Err(e)) = (!fronts.is_empty()).then(|| shim.check()) {
        log(sink, &format!("fronts not started: {e}"));
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
        .unzip::<_, _, Vec<_>, Vec<_>>();
    let (mut stops, mut threads) = (stops, threads);
    if let Some((program, plan)) = proxy {
        let (stop, stopped) = mpsc::channel();
        let sink = sink.clone();
        stops.push(stop);
        threads.push(thread::spawn(move || {
            front_proxy::keep_running(&plan, &program, &stopped, &sink);
        }));
    }
    Supervised { stops, threads }
}

/// Runs `front` until told to stop, restarting it with backoff whenever it exits.
#[expect(
    clippy::disallowed_methods,
    reason = "times how long a child ran to pick its restart backoff, not domain time"
)]
fn keep_running(front: &Front, shim: &Shim, stop: &Receiver<Stop>, sink: &Sink) {
    let ns = &front.ns;
    #[cfg(target_os = "linux")]
    leftover::stop(front, sink);
    if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
        return;
    }
    let mut backoff = FIRST_BACKOFF;
    loop {
        let started = Instant::now();
        let exited = match spawn(front, shim, sink) {
            Ok(mut child) => {
                let outcome = watch(&mut child, stop);
                let _ = fs::remove_file(front.pid_file());
                let Outcome::Exited(status) = outcome else {
                    return;
                };
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

/// How a watched front ended.
pub(super) enum Outcome {
    /// It exited on its own.
    Exited(ExitStatus),
    /// It was told to stop, and is gone.
    Stopped,
}

/// Waits for `child` to exit. On [`Stop::Term`] it sends SIGTERM and keeps waiting, killing the
/// child once the grace is over; on [`Stop::Kill`] it kills the child at once.
#[expect(
    clippy::disallowed_methods,
    reason = "times a grace period for a child, not domain time"
)]
pub(super) fn watch(child: &mut Child, stop: &Receiver<Stop>) -> Outcome {
    let mut kill_at: Option<Instant> = None;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return if kill_at.is_some() {
                Outcome::Stopped
            } else {
                Outcome::Exited(status)
            };
        }
        if kill_at.is_some_and(|at| Instant::now() >= at) {
            return kill(child);
        }
        match stop.recv_timeout(POLL) {
            Ok(Stop::Term(grace)) if kill_at.is_none() => {
                let _ = kill_process(Pid::from_child(child), Signal::TERM);
                kill_at = Some(Instant::now() + grace);
            }
            Ok(Stop::Term(_)) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Stop::Kill) | Err(RecvTimeoutError::Disconnected) => return kill(child),
        }
    }
}

/// Kills and reaps `child`. On macOS the shim leads a process group that holds its front, and
/// the whole group is killed.
fn kill(child: &mut Child) -> Outcome {
    #[cfg(target_os = "macos")]
    let _ = rustix::process::kill_process_group(Pid::from_child(child), Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
    Outcome::Stopped
}

/// Starts `front` under `shim` in `/`, with its socket in `MOOR_FRONT_SOCKET`, and relays its
/// stdout and stderr lines to `sink`. The shim exits with this controller. A front with a
/// state dir may write it, finds it in `MOOR_FRONT_STATE_DIR` and keeps its temporary files in
/// its `tmp`. On Linux the socket is on fd 3 and the shim's pid is recorded; macOS has no
/// `O_PATH`, so there the front dials the socket's path, and the shim leads a process group.
fn spawn(front: &Front, shim: &Shim, sink: &Sink) -> io::Result<Child> {
    let ns = &front.ns;
    let mut command = shim.command();
    command.args(["--parent", &process::id().to_string()]);
    if let Some(dir) = &front.state_dir {
        let tmp = dir.join("tmp");
        DirBuilder::new().recursive(true).mode(0o700).create(&tmp)?;
        command
            .arg("--writable")
            .arg(dir)
            .env("MOOR_FRONT_STATE_DIR", dir)
            .env("TMPDIR", tmp);
    }
    for path in &front.read_only {
        command.arg("--read-only").arg(path);
    }
    if let Some(proxy) = &front.proxy {
        command
            .arg("--proxy")
            .arg(&proxy.socket)
            .env(front_proxy::CA_VAR, &proxy.ca);
    }
    command
        .arg("--")
        .args(&front.command)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        let socket =
            rustix::fs::open(&front.socket, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())?;
        command
            .env("MOOR_FRONT_SOCKET", format!("/proc/self/fd/{SOCKET_FD}"))
            .fd_mappings(vec![FdMapping {
                parent_fd: socket,
                child_fd: SOCKET_FD,
            }])
            .map_err(io::Error::other)?;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt as _;
        command
            .env("MOOR_FRONT_SOCKET", &front.socket)
            .process_group(0);
    }
    let mut child = command.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        relay_lines(stdout, ns, sink);
    }
    if let Some(stderr) = child.stderr.take() {
        relay_lines(stderr, ns, sink);
    }
    let pid = child.id();
    #[cfg(target_os = "macos")]
    log(sink, &format!("front {ns}: started pid {pid}"));
    #[cfg(target_os = "linux")]
    match write_pid_file(front, pid) {
        Ok(()) => log(sink, &format!("front {ns}: started pid {pid}")),
        Err(e) => log(
            sink,
            &format!("front {ns}: started pid {pid}; no pid file: {e}"),
        ),
    }
    Ok(child)
}

/// Replaces the pid file of `front` with `pid` and its start time.
#[cfg(target_os = "linux")]
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
