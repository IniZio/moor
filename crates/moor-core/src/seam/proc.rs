//! Proc seam: the single spawner every adapter shells out through.

use crate::secret::Secret;
use std::{ffi::OsString, fmt, path::PathBuf, time::Duration};
mod error;
mod first_line;
mod gate;
mod signal;
pub use error::ProcError;
pub use first_line::FirstLineError;
pub use gate::StderrGate;
pub use signal::{check_group, Signal};

/// An external program moor may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// The crabbox driver hub.
    Crabbox,
    /// The herdr surface.
    Herdr,
    /// The agent engine program (claude today).
    Agent,
    /// OpenSSH client.
    Ssh,
    /// GitHub CLI.
    Gh,
    /// Git.
    Git,
    /// Rsync.
    Rsync,
    /// This binary, re-invoked.
    MoorSelf,
    /// POSIX shell, for credential `cmd` sources.
    Sh,
}

impl Tool {
    /// Environment variable that overrides the program path.
    #[must_use]
    pub const fn bin_var(self) -> &'static str {
        match self {
            Self::Crabbox => "MOOR_CRABBOX_BIN",
            Self::Herdr => "MOOR_HERDR_BIN",
            Self::Agent => "MOOR_CLAUDE_BIN",
            Self::Ssh => "MOOR_SSH_BIN",
            Self::Gh => "MOOR_GH_BIN",
            Self::Git => "MOOR_GIT_BIN",
            Self::Rsync => "MOOR_RSYNC_BIN",
            Self::MoorSelf => "MOOR_SELF_BIN",
            Self::Sh => "MOOR_SH_BIN",
        }
    }

    /// Default program name looked up on `PATH`.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Crabbox => "crabbox",
            Self::Herdr => "herdr",
            Self::Agent => "claude",
            Self::Ssh => "ssh",
            Self::Gh => "gh",
            Self::Git => "git",
            Self::Rsync => "rsync",
            Self::MoorSelf => "moor",
            Self::Sh => "sh",
        }
    }
}

/// Environment variables every child inherits; only crabbox gets more, see
/// [`CRABBOX_ENV`] and [`CRABBOX_ENV_PREFIX`].
pub const BASE_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "SSH_AUTH_SOCK",
    "GIT_CEILING_DIRECTORIES",
];

/// Extra host variables crabbox inherits so it can reach a local container
/// runtime and find its config.
pub const CRABBOX_ENV: &[&str] = &[
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "USER",
];

/// Host variables with this name prefix are also inherited by crabbox.
pub const CRABBOX_ENV_PREFIX: &str = "CRABBOX_";

/// Per-call environment additions on top of [`BASE_ENV`].
#[derive(Clone, Debug, Default)]
pub struct EnvAllow {
    /// Extra name/value pairs passed to the child.
    pub extra: Vec<(String, String)>,
}

impl EnvAllow {
    /// Adds one variable.
    #[must_use]
    pub fn with(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra.push((k.into(), v.into()));
        self
    }
}

/// What a child reads on stdin.
pub enum Stdin {
    /// `/dev/null`.
    Null,
    /// Plain bytes.
    Bytes(Vec<u8>),
    /// A secret written to the pipe.
    Secret(Secret),
    /// A secret-bearing file that must be mode 0600.
    File0600(PathBuf),
}

impl fmt::Debug for Stdin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bytes(b) => write!(f, "Bytes({} bytes)", b.len()),
            Self::Secret(s) => write!(f, "Secret({})", s.digest().as_str()),
            Self::File0600(p) => f.debug_tuple("File0600").field(p).finish(),
        }
    }
}

/// Wall-clock limit for one child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline(pub Duration);

/// A fully described child process.
#[derive(Debug)]
pub struct ProcSpec {
    /// Program to run.
    pub tool: Tool,
    /// Arguments, never secrets.
    pub args: Vec<OsString>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Environment additions.
    pub env: EnvAllow,
    /// Stdin source.
    pub stdin: Stdin,
    /// Time limit.
    pub deadline: Deadline,
    /// Only stderr lines strictly between the begin and end marker lines are forwarded or logged by `Proc::run_streamed`; other calls ignore it.
    pub stderr_gate: Option<StderrGate>,
}

impl ProcSpec {
    /// Starts a spec with no args, null stdin and the given deadline.
    #[must_use]
    pub fn new(tool: Tool, deadline: Deadline) -> Self {
        Self {
            tool,
            args: Vec::new(),
            cwd: None,
            env: EnvAllow::default(),
            stdin: Stdin::Null,
            deadline,
            stderr_gate: None,
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Appends several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, it: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(it.into_iter().map(Into::into));
        self
    }

    /// Sets the working directory.
    #[must_use]
    pub fn cwd(mut self, p: impl Into<PathBuf>) -> Self {
        self.cwd = Some(p.into());
        self
    }

    /// Adds one environment variable.
    #[must_use]
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env = self.env.with(k, v);
        self
    }

    /// Sets the stdin source.
    #[must_use]
    pub fn stdin(mut self, s: Stdin) -> Self {
        self.stdin = s;
        self
    }

    /// Arguments as lossy UTF-8 strings.
    #[must_use]
    pub fn arg_strings(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
}

/// Where a streamed child's stdout and stderr go; never captured by moor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// moor's own stdout and stderr.
    Inherit,
    /// One file receiving both streams, created or truncated.
    Log(PathBuf),
}

/// Bytes a Proc keeps of each output stream; the rest is read and discarded.
pub const OUTPUT_CAP: usize = 4 * 1024 * 1024;

/// Captured result of a child that exited 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcOutput {
    /// Exit code.
    pub code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Output past the per-stream cap was discarded.
    pub truncated: bool,
}

/// How a long-lived child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code, when the child exited normally.
    pub code: Option<i32>,
    /// Signal number, when a signal ended the child.
    pub signal: Option<i32>,
}

/// A running child in its own process group. Dropping the handle sends SIGTERM
/// to the group, waits a short grace, sends SIGKILL, then reaps the leader.
pub trait ChildHandle: Send {
    /// Leader pid, which is also the process group id.
    fn pid(&self) -> u32;

    /// First stdout line without its line ending; later calls return it at once.
    ///
    /// # Errors
    /// [`FirstLineError::Timeout`] when no line arrives within `deadline`,
    /// [`FirstLineError::Exited`] when the child exits first,
    /// [`FirstLineError::Io`] when stdout cannot be read.
    fn first_line(&mut self, deadline: Deadline) -> Result<String, FirstLineError>;

    /// How the child ended, or `None` while it runs. Never blocks or probes by
    /// pid, so a reused pid cannot answer for this child.
    fn try_exit(&mut self) -> Option<ExitInfo>;
}

/// Runs external programs; the only place moor spawns processes.
pub trait Proc: Send + Sync {
    /// Runs to completion; `Ok` only on exit 0.
    ///
    /// # Errors
    /// Returns [`ProcError`] on spawn failure, timeout, signal or non-zero exit.
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError>;

    /// Runs to completion and returns raw stdout bytes, never decoded. Errors
    /// never carry stdout: [`ProcError::NonZero`] has an empty `stdout`.
    ///
    /// # Errors
    /// [`ProcError`] on spawn failure, timeout, signal, non-zero exit or oversize output.
    fn run_secret(&self, spec: &ProcSpec) -> Result<zeroize::Zeroizing<Vec<u8>>, ProcError>;

    /// Runs to completion with output streamed to `out`; interrupting moor
    /// stops the child's process group. A non-zero exit is
    /// [`ProcError::NonZero`] with empty `stdout` and the stderr tail. With
    /// `spec.stderr_gate` set only gated stderr is shown; the tail keeps every line.
    ///
    /// # Errors
    /// [`ProcError`] on spawn failure, timeout, signal, interrupt or non-zero exit.
    fn run_streamed(&self, spec: &ProcSpec, out: &Output) -> Result<(), ProcError>;

    /// Runs to completion, handing raw stdout to `read` as it arrives; stdout is never held whole.
    /// `read` is called once after the child starts and may stop before EOF; the rest of stdout is
    /// discarded. A non-zero exit is [`ProcError::NonZero`] with empty `stdout` and the stderr tail.
    ///
    /// # Errors
    /// [`ProcError`] on spawn failure, timeout, signal, interrupt or non-zero exit.
    fn run_reading(
        &self,
        spec: &ProcSpec,
        read: &mut dyn FnMut(&mut dyn std::io::Read),
    ) -> Result<(), ProcError> {
        let _ = (spec, read);
        Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "streamed stdout is not supported",
        )))
    }

    /// Starts a child and leaves it running.
    ///
    /// # Errors
    /// Returns [`ProcError::Spawn`] when the program cannot start.
    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError>;

    /// Starts a child in its own process group with stdin and stdout null and
    /// stderr appended to `log`, without waiting, so it outlives moor; returns
    /// its pid.
    ///
    /// # Errors
    /// [`ProcError::Spawn`] when it cannot start or daemons are unsupported.
    fn spawn_daemon(&self, spec: &ProcSpec, log: &std::path::Path) -> Result<u32, ProcError> {
        let _ = (spec, log);
        Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "detached daemons are not supported",
        )))
    }

    /// Starts a long-lived child in its own process group with stdout piped
    /// for [`ChildHandle::first_line`] and stderr kept for its tail.
    /// `spec.deadline` does not limit the child's lifetime.
    ///
    /// # Errors
    /// [`ProcError::Spawn`] when it cannot start or children are unsupported.
    fn spawn_child(&self, spec: &ProcSpec) -> Result<Box<dyn ChildHandle>, ProcError> {
        let _ = spec;
        Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "long-lived children are not supported",
        )))
    }

    /// Sends `sig` to process group `pgid`; `Err` when it is gone or refused.
    ///
    /// # Errors
    /// [`ProcError`] when the group cannot be signalled or is unsupported.
    fn signal_group(&self, pgid: u32, sig: Signal) -> Result<(), ProcError> {
        let _ = (pgid, sig);
        Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "signalling process groups is not supported",
        )))
    }

    /// Identifier of the current host boot; `None` where the host has none.
    fn boot_id(&self) -> Option<String> {
        None
    }

    /// Whether `pid` exists and leads its own process group (its pgid is its pid).
    fn group_leader(&self, pid: u32) -> bool {
        let _ = pid;
        false
    }

    /// Whether a process with this pid exists.
    fn alive(&self, pid: u32) -> bool;

    /// The program this runs for `tool`, for a tool that starts another, as rsync starts ssh.
    fn program_path(&self, tool: Tool) -> Option<std::ffi::OsString> {
        let _ = tool;
        None
    }
}

#[cfg(test)]
mod tests;
