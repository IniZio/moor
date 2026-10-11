//! `SystemProc`: the real spawner every adapter shells out through.
#![cfg_attr(not(test), expect(dead_code, reason = "wired by S3 wire.rs"))]

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use moor_core::seam::proc::{Proc, ProcError, ProcOutput, ProcSpec, Stdin, Tool, BASE_ENV};
use rustix::process::{kill_process_group, Pid, Signal};

use crate::env::Env;

/// Longest wait between SIGTERM and SIGKILL.
const GRACE: Duration = Duration::from_secs(2);
/// Poll interval while waiting for a child.
const POLL: Duration = Duration::from_millis(10);
/// Spawn attempts when the program file is briefly busy (`ETXTBSY`).
const SPAWN_TRIES: u32 = 8;
/// Bytes of stderr kept in `NonZero`.
const TAIL: usize = 2048;

/// Every tool, for program resolution at construction.
const TOOLS: [Tool; 8] = [
    Tool::Crabbox,
    Tool::Herdr,
    Tool::Agent,
    Tool::Ssh,
    Tool::Gh,
    Tool::Git,
    Tool::Rsync,
    Tool::MoorSelf,
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
            Tool::Rsync | Tool::MoorSelf => None,
        }
    }
}

/// Spawns real processes with a cleared environment and process-group kills.
#[derive(Debug)]
pub(crate) struct SystemProc {
    /// Base variables captured at construction.
    base: Vec<(String, String)>,
    /// Resolved program per tool.
    programs: Vec<(Tool, OsString)>,
}

impl SystemProc {
    /// Captures base variables and resolves programs: env var, config, then `PATH`.
    pub(crate) fn new(env: &Env, tools: &ToolPaths) -> Self {
        let base = BASE_ENV
            .iter()
            .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
            .collect();
        let programs = TOOLS.iter().map(|&t| (t, resolve(env, tools, t))).collect();
        Self { base, programs }
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
        cmd.envs(self.base.iter().map(|(k, v)| (k, v)));
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

/// Program for `tool`: `MOOR_<TOOL>_BIN`, then config, then the default name.
fn resolve(env: &Env, tools: &ToolPaths, tool: Tool) -> OsString {
    if let Some(v) = env.get(tool.bin_var()).filter(|v| !v.is_empty()) {
        return v.into();
    }
    tools
        .get(tool)
        .map_or_else(|| tool.program().into(), |p| p.clone().into_os_string())
}

/// Stdio for the child plus bytes to write to its pipe, if any.
fn stdin_for(stdin: &Stdin) -> io::Result<(Stdio, Option<&[u8]>)> {
    match stdin {
        Stdin::Null => Ok((Stdio::null(), None)),
        Stdin::Bytes(b) => Ok((Stdio::piped(), Some(b.as_slice()))),
        Stdin::Secret(s) => Ok((Stdio::piped(), Some(s.expose()))),
        Stdin::File0600(p) => {
            let file = File::open(p)?;
            if file.metadata()?.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "secret file must be mode 0600",
                ));
            }
            Ok((Stdio::from(file), None))
        }
    }
}

/// Reads a pipe to its end.
fn drain<R: Read>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut r) = pipe {
        drop(r.read_to_end(&mut buf));
    }
    buf
}

/// Writes `bytes` to the pipe and closes it.
fn feed<W: Write>(pipe: Option<W>, bytes: Option<&[u8]>) {
    if let (Some(mut w), Some(b)) = (pipe, bytes) {
        drop(w.write_all(b));
    }
}

/// Sends `sig` to the process group led by `pid`; errors mean the group is gone.
fn signal_group(pid: u32, sig: Signal) {
    let pid = i32::try_from(pid).ok().and_then(Pid::from_raw);
    if let Some(pid) = pid {
        kill_process_group(pid, sig).ok();
    }
}

/// Result of reaping the child.
type Waited = io::Result<ExitStatus>;

/// Error for a waiter thread that died without reporting.
fn gone() -> io::Error {
    io::Error::other("child waiter vanished")
}

/// SIGTERM the group, wait a short grace, SIGKILL the group, reap the leader.
fn terminate(pid: u32, rx: &Receiver<Waited>, deadline: Duration) -> io::Result<()> {
    signal_group(pid, Signal::TERM);
    let grace = GRACE.min(deadline.max(POLL));
    let exited = match rx.recv_timeout(grace) {
        Ok(st) => {
            st?;
            true
        }
        Err(RecvTimeoutError::Timeout) => false,
        Err(RecvTimeoutError::Disconnected) => return Err(gone()),
    };
    signal_group(pid, Signal::KILL);
    if !exited {
        rx.recv().map_err(|_| gone())??;
    }
    Ok(())
}

/// Maps a finished child plus captured output to the seam result.
fn finish(status: ExitStatus, out: &[u8], err: &[u8]) -> Result<ProcOutput, ProcError> {
    let stdout = String::from_utf8_lossy(out).into_owned();
    match status.code() {
        Some(0) => Ok(ProcOutput {
            code: 0,
            stdout,
            stderr: String::from_utf8_lossy(err).into_owned(),
        }),
        Some(code) => {
            let tail = err
                .get(err.len().saturating_sub(TAIL)..)
                .unwrap_or_default();
            Err(ProcError::NonZero {
                code,
                stdout,
                stderr_tail: String::from_utf8_lossy(tail).into_owned(),
            })
        }
        None => Err(ProcError::Signalled(status.signal().unwrap_or(0))),
    }
}

impl Proc for SystemProc {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        let (w, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let limit = spec.deadline.0;
        thread::scope(|s| {
            s.spawn(|| feed(w, payload));
            let out = s.spawn(|| drain(o));
            let err = s.spawn(|| drain(e));
            let pid = child.id();
            let (tx, rx) = mpsc::channel();
            s.spawn(move || tx.send(child.wait()).ok());
            let status = match rx.recv_timeout(limit) {
                Ok(st) => st.map_err(ProcError::Spawn)?,
                Err(RecvTimeoutError::Timeout) => {
                    terminate(pid, &rx, limit).map_err(ProcError::Spawn)?;
                    return Err(ProcError::Timeout {
                        tool: spec.tool,
                        after: limit,
                    });
                }
                Err(RecvTimeoutError::Disconnected) => return Err(ProcError::Spawn(gone())),
            };
            let out = out.join().unwrap_or_default();
            let err = err.join().unwrap_or_default();
            finish(status, &out, &err)
        })
    }

    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError> {
        let mut cmd = self.command(spec);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        thread::spawn(move || drop(child.wait()));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
