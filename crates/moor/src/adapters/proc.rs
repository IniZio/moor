//! `SystemProc`: the real spawner every adapter shells out through.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::seam::proc::{
    ChildHandle, Output, Proc, ProcError, ProcOutput, ProcSpec, Signal as GroupSignal, Tool,
};
use rustix::io::Errno;
use rustix::process::test_kill_process;
use zeroize::Zeroizing;

use crate::env::Env;
use env::{resolve, Snapshot};
use grace::{terminate, GRACE};
use signal::{is_group_leader, send_group, signal_group, to_pid};
use stdin::stdin_for;
use trace::Trace;

/// Poll interval while waiting for a child.
const POLL: Duration = Duration::from_millis(10);
/// Spawn attempts when the program file is briefly busy (`ETXTBSY`).
const SPAWN_TRIES: u32 = 8;
/// Bytes of stderr kept in `NonZero`.
const TAIL: usize = 2048;
/// Longest extra wait for pipe readers after the child exits (an escaped
/// grandchild may hold the pipe open), shared by both pipes.
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// Bytes kept per stream; the rest is read and discarded.
const CAP: usize = 4 * 1024 * 1024;

/// Every tool, for program resolution at construction.
const TOOLS: [Tool; 9] = [
    Tool::Crabbox,
    Tool::Herdr,
    Tool::Agent,
    Tool::Ssh,
    Tool::Gh,
    Tool::Git,
    Tool::Rsync,
    Tool::MoorSelf,
    Tool::Sh,
];

/// Program paths from config `tools.*`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ToolPaths {
    /// Crabbox program.
    pub crabbox: Option<PathBuf>,
    /// Herdr program.
    pub herdr: Option<PathBuf>,
    /// Agent engine program.
    pub claude: Option<PathBuf>,
    /// GitHub CLI program.
    pub gh: Option<PathBuf>,
    /// OpenSSH client program.
    pub ssh: Option<PathBuf>,
    /// Git program.
    pub git: Option<PathBuf>,
}

impl ToolPaths {
    /// Configured path for `tool`, if any.
    const fn get(&self, tool: Tool) -> Option<&PathBuf> {
        match tool {
            Tool::Crabbox => self.crabbox.as_ref(),
            Tool::Herdr => self.herdr.as_ref(),
            Tool::Agent => self.claude.as_ref(),
            Tool::Gh => self.gh.as_ref(),
            Tool::Ssh => self.ssh.as_ref(),
            Tool::Git => self.git.as_ref(),
            Tool::Rsync | Tool::MoorSelf | Tool::Sh => None,
        }
    }
}

/// Spawns real processes with a cleared environment and process-group kills.
#[derive(Debug)]
pub(crate) struct SystemProc {
    /// Environment captured at construction.
    env: Snapshot,
    /// Resolved program per tool.
    programs: Vec<(Tool, OsString)>,
    /// Opt-in per-call crabbox timing.
    trace: Trace,
}

impl SystemProc {
    /// Captures base variables and resolves programs: env var, config, then `PATH`.
    pub(crate) fn new(env: &Env, tools: &ToolPaths) -> Self {
        let programs = TOOLS.iter().map(|&t| (t, resolve(env, tools, t))).collect();
        Self {
            env: Snapshot::new(env),
            programs,
            trace: Trace::new(env),
        }
    }

    /// Program for `tool`.
    fn program(&self, tool: Tool) -> OsString {
        self.programs
            .iter()
            .find(|(t, _)| *t == tool)
            .map_or_else(|| tool.program().into(), |(_, p)| p.clone())
    }

    /// Command with program, args, cwd, cleared env and its own process group.
    fn command(&self, spec: &ProcSpec) -> Command {
        let mut cmd = Command::new(self.program(spec.tool));
        cmd.args(&spec.args).env_clear().process_group(0);
        self.env.apply(&mut cmd, spec.tool);
        cmd.envs(spec.env.extra.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }
}

/// Spawns, retrying with short backoff while the executable is busy: another
/// thread may hold a write fd to it across its own fork.
fn spawn_retrying(cmd: &mut Command) -> io::Result<Child> {
    let mut tries = 1;
    loop {
        match cmd.spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && tries < SPAWN_TRIES => {
                thread::sleep(POLL * tries);
                tries += 1;
            }
            r => return r,
        }
    }
}

/// Bytes read so far from one pipe.
#[derive(Default)]
struct Capture {
    /// Kept bytes, at most [`CAP`]; wiped on drop.
    buf: Zeroizing<Vec<u8>>,
    /// Bytes past [`CAP`] were discarded.
    truncated: bool,
}

/// Detached reader: owns the pipe, fills the shared capture, signals EOF on `done`.
fn drain<R: Read + Send + 'static>(pipe: Option<R>, done: Sender<bool>) -> Arc<Mutex<Capture>> {
    let cap = Arc::new(Mutex::new(Capture::default()));
    let shared = Arc::clone(&cap);
    thread::spawn(move || {
        if let Some(mut r) = pipe {
            let mut chunk = [0_u8; 8192];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut c) = shared.lock() {
                    let room = CAP.saturating_sub(c.buf.len());
                    let take = n.min(room);
                    c.buf
                        .extend_from_slice(chunk.get(..take).unwrap_or_default());
                    c.truncated |= take < n;
                }
            }
        }
        done.send(true).ok();
    });
    cap
}

/// Waits for both readers' EOF (`true`) or the [`DRAIN_GRACE`] tick (`false`).
fn wait_eof(events: &Receiver<bool>, tx: Sender<bool>) {
    thread::spawn(move || {
        thread::sleep(DRAIN_GRACE);
        tx.send(false).ok();
    });
    let mut eofs = 0;
    while eofs < 2 {
        match events.recv() {
            Ok(true) => eofs += 1,
            Ok(false) | Err(_) => break,
        }
    }
}

/// Takes what was read so far.
fn take(cap: &Mutex<Capture>) -> Capture {
    cap.lock()
        .map(|mut c| std::mem::take(&mut *c))
        .unwrap_or_default()
}

/// Writes `bytes` to the pipe and closes it.
fn feed<W: Write>(pipe: Option<W>, bytes: Option<&[u8]>) {
    if let (Some(mut w), Some(b)) = (pipe, bytes) {
        drop(w.write_all(b));
    }
}

/// Result of reaping the child.
type Waited = io::Result<ExitStatus>;

/// Error for a waiter thread that died without reporting.
fn gone() -> io::Error {
    io::Error::other("child waiter vanished")
}

/// Error for a child that exited without success; never carries stdout.
fn failure(status: ExitStatus, err: &[u8]) -> ProcError {
    status.code().map_or_else(
        || ProcError::Signalled(status.signal().unwrap_or(0)),
        |code| {
            let tail = err
                .get(err.len().saturating_sub(TAIL)..)
                .unwrap_or_default();
            ProcError::NonZero {
                code,
                stdout: String::new(),
                stderr_tail: String::from_utf8_lossy(tail).into_owned(),
            }
        },
    )
}

/// Maps a finished child plus captured output to the seam result.
fn finish(status: ExitStatus, o: &Capture, e: &Capture) -> Result<ProcOutput, ProcError> {
    if status.code() == Some(0) {
        return Ok(ProcOutput {
            code: 0,
            stdout: String::from_utf8_lossy(&o.buf).into_owned(),
            stderr: String::from_utf8_lossy(&e.buf).into_owned(),
            truncated: o.truncated || e.truncated,
        });
    }
    Err(match failure(status, &e.buf) {
        ProcError::NonZero {
            code, stderr_tail, ..
        } => ProcError::NonZero {
            code,
            stdout: String::from_utf8_lossy(&o.buf).into_owned(),
            stderr_tail,
        },
        other => other,
    })
}

impl SystemProc {
    /// Spawns, feeds stdin, enforces the deadline and returns the exit status
    /// with both captured streams.
    fn exec(&self, spec: &ProcSpec) -> Result<(ExitStatus, Capture, Capture), ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        let (w, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let limit = spec.deadline.0;
        let payload = payload.map(|b| Zeroizing::new(b.to_vec()));
        thread::spawn(move || feed(w, payload.as_ref().map(|p| p.as_slice())));
        let (etx, erx) = mpsc::channel();
        let out = drain(o, etx.clone());
        let err = drain(e, etx.clone());
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || tx.send(child.wait()).ok());
        let status = match rx.recv_timeout(limit) {
            Ok(st) => st.map_err(ProcError::Spawn)?,
            Err(RecvTimeoutError::Timeout) => {
                terminate(pid, &rx, spec.tool, limit).map_err(ProcError::Spawn)?;
                wait_eof(&erx, etx);
                return Err(ProcError::Timeout {
                    tool: spec.tool,
                    after: limit,
                });
            }
            Err(RecvTimeoutError::Disconnected) => return Err(ProcError::Spawn(gone())),
        };
        wait_eof(&erx, etx);
        Ok((status, take(&out), take(&err)))
    }
}

impl SystemProc {
    /// Stdout of a zero exit; truncated output is an error, never a short secret.
    fn secret(&self, spec: &ProcSpec) -> Result<Zeroizing<Vec<u8>>, ProcError> {
        let (status, out, err) = self.exec(spec)?;
        if status.code() != Some(0) {
            return Err(failure(status, &err.buf));
        }
        if out.truncated {
            return Err(ProcError::Spawn(io::Error::other(
                "secret output exceeds the capture limit",
            )));
        }
        Ok(out.buf)
    }
}

impl Proc for SystemProc {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        self.trace.timed(spec, || {
            let (status, out, err) = self.exec(spec)?;
            finish(status, &out, &err)
        })
    }

    /// Stdout that hit the capture cap is [`ProcError::Spawn`] (an I/O error),
    /// never a truncated secret.
    fn run_secret(&self, spec: &ProcSpec) -> Result<Zeroizing<Vec<u8>>, ProcError> {
        self.trace.timed(spec, || self.secret(spec))
    }

    fn run_streamed(&self, spec: &ProcSpec, out: &Output) -> Result<(), ProcError> {
        self.trace.timed(spec, || self.stream_forwarding(spec, out))
    }
    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError> {
        let mut cmd = self.command(spec);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        thread::spawn(move || drop(child.wait()));
        self.trace.started(spec);
        Ok(())
    }

    fn spawn_child(&self, spec: &ProcSpec) -> Result<Box<dyn ChildHandle>, ProcError> {
        self.spawn_long_lived(spec)
            .inspect(|_| self.trace.started(spec))
    }

    fn spawn_daemon(&self, spec: &ProcSpec, log: &Path) -> Result<u32, ProcError> {
        self.daemon(spec, log).inspect(|_| self.trace.started(spec))
    }

    fn signal_group(&self, pgid: u32, sig: GroupSignal) -> Result<(), ProcError> {
        send_group(pgid, sig)
    }

    fn boot_id(&self) -> Option<String> {
        boot::read()
    }

    fn group_leader(&self, pid: u32) -> bool {
        is_group_leader(pid)
    }

    fn alive(&self, pid: u32) -> bool {
        to_pid(pid).is_some_and(|p| matches!(test_kill_process(p), Ok(()) | Err(Errno::PERM)))
    }
}

mod boot;
mod child;
mod detached;
mod env;
mod grace;
mod signal;
mod stdin;
mod stream;
#[cfg(test)]
mod tests;
mod trace;
