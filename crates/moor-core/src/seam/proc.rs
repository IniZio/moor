//! Proc seam: the single spawner every adapter shells out through.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::secret::Secret;

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

/// Environment variables every child inherits; nothing else is inherited.
pub const BASE_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "SSH_AUTH_SOCK",
    "GIT_CEILING_DIRECTORIES",
];

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

/// Why a child did not succeed.
#[derive(Debug)]
pub enum ProcError {
    /// The program could not be started.
    Spawn(std::io::Error),
    /// The deadline passed and the child was killed.
    Timeout {
        /// Program that timed out.
        tool: Tool,
        /// Deadline that elapsed.
        after: Duration,
    },
    /// The child died from a signal.
    Signalled(i32),
    /// moor was interrupted and the child was stopped.
    Interrupted,
    /// The child exited non-zero.
    NonZero {
        /// Exit code.
        code: i32,
        /// Captured stdout.
        stdout: String,
        /// Last ~2 KiB of stderr.
        stderr_tail: String,
    },
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start program: {e}"),
            Self::Timeout { tool, after } => {
                write!(f, "{} timed out after {}s", tool.program(), after.as_secs())
            }
            Self::Signalled(s) => write!(f, "program killed by signal {s}"),
            Self::Interrupted => f.write_str("interrupted"),
            Self::NonZero { code, .. } => write!(f, "program exited with code {code}"),
        }
    }
}

impl std::error::Error for ProcError {}

/// Why a long-lived child gave no first stdout line.
#[derive(Debug)]
pub enum FirstLineError {
    /// No full line arrived before the deadline; the child keeps running.
    Timeout {
        /// Program.
        tool: Tool,
        /// Deadline that elapsed.
        after: Duration,
    },
    /// The child exited before writing a full line.
    Exited {
        /// Exit code, if it exited normally.
        code: Option<i32>,
        /// Signal, if killed by one.
        signal: Option<i32>,
        /// Last ~2 KiB of stderr.
        stderr_tail: String,
    },
    /// Reading the child's stdout failed.
    Io(std::io::Error),
}

impl fmt::Display for FirstLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { tool, after } => write!(
                f,
                "{} gave no first line within {}s",
                tool.program(),
                after.as_secs()
            ),
            Self::Exited { code: Some(c), .. } => {
                write!(f, "program exited with code {c} before its first line")
            }
            Self::Exited {
                signal: Some(s), ..
            } => write!(f, "program killed by signal {s} before its first line"),
            Self::Exited { .. } => f.write_str("program exited before its first line"),
            Self::Io(e) => write!(f, "could not read program output: {e}"),
        }
    }
}

impl std::error::Error for FirstLineError {}

/// A running child in its own process group. Dropping the handle sends
/// SIGTERM to the group, then SIGKILL after a short grace, and reaps the
/// leader.
pub trait ChildHandle: Send {
    /// Leader pid, which is also the process group id.
    fn pid(&self) -> u32;

    /// First stdout line without its line ending. Once a line arrived, later
    /// calls return the same line at once.
    ///
    /// # Errors
    /// [`FirstLineError::Timeout`] when no line arrives within `deadline`,
    /// [`FirstLineError::Exited`] when the child exits first,
    /// [`FirstLineError::Io`] when stdout cannot be read.
    fn first_line(&mut self, deadline: Deadline) -> Result<String, FirstLineError>;
}

/// Runs external programs; the only place moor spawns processes.
pub trait Proc: Send + Sync {
    /// Runs to completion; `Ok` only on exit 0.
    ///
    /// # Errors
    /// Returns [`ProcError`] on spawn failure, timeout, signal or non-zero exit.
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError>;

    /// Runs to completion and returns raw stdout bytes, never decoded.
    ///
    /// Errors never carry stdout: [`ProcError::NonZero`] has an empty `stdout`.
    ///
    /// # Errors
    /// Returns [`ProcError`] on spawn failure, timeout, signal, non-zero exit
    /// or output that does not fit the capture limit.
    fn run_secret(&self, spec: &ProcSpec) -> Result<zeroize::Zeroizing<Vec<u8>>, ProcError>;

    /// Runs to completion with output streamed to `out`. Interrupting moor
    /// stops the child's process group.
    ///
    /// A non-zero exit is [`ProcError::NonZero`] with an empty `stdout` and the
    /// tail of stderr, which is still streamed to `out`.
    ///
    /// # Errors
    /// Returns [`ProcError`] on spawn failure, timeout, signal, interrupt or
    /// non-zero exit.
    fn run_streamed(&self, spec: &ProcSpec, out: &Output) -> Result<(), ProcError>;

    /// Starts a child and leaves it running.
    ///
    /// # Errors
    /// Returns [`ProcError::Spawn`] when the program cannot start.
    fn spawn_detached(&self, spec: &ProcSpec) -> Result<(), ProcError>;

    /// Starts a long-lived child in its own process group with stdout piped
    /// for [`ChildHandle::first_line`] and stderr kept for its tail.
    /// `spec.deadline` does not limit the child's lifetime.
    ///
    /// # Errors
    /// Returns [`ProcError::Spawn`] when the program cannot start, or when
    /// this spawner cannot run long-lived children.
    fn spawn_child(&self, spec: &ProcSpec) -> Result<Box<dyn ChildHandle>, ProcError> {
        let _ = spec;
        Err(ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "long-lived children are not supported",
        )))
    }

    /// Whether a process with this pid exists.
    fn alive(&self, pid: u32) -> bool;
}

#[cfg(test)]
mod tests;
