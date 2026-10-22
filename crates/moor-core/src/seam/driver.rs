//! Driver seam: how moor gets, inspects and releases Boxes.
//!
//! Provisioning, exec, copy-out, file peek and put, connect argv, reverse tunnel, sleep
//! and wake. A reverse tunnel is a long-lived child that the caller owns and drops to stop.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use crate::name::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};
use crate::seam::proc::{Deadline, Output};

mod batch;
mod tunnel;

pub use batch::{run_each, BoxStep, StepDone};
use tunnel::fmt_tunnel;
pub use tunnel::{TunnelFailure, TunnelHandle};

/// How a provider keeps an idle Box cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepKind {
    /// The Driver suspends the Box.
    Suspend,
    /// The provider sleeps the Box on its own.
    Native,
    /// The Box cannot sleep.
    None,
}

/// Facts about a provider, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCaps {
    /// Sleep style.
    pub sleep: SleepKind,
    /// Whether a sleeping Box can wake.
    pub wake: bool,
    /// Box-to-host tunnel support.
    pub reverse_tunnel: bool,
    /// Whether the Box reports a working directory.
    pub has_workdir: bool,
}

/// A request for a new Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireReq {
    /// Provider to ask.
    pub provider: ProviderId,
    /// Source for the per-Box name the adapter derives.
    pub box_id: BoxId,
    /// Box name the adapter embeds in the Box name.
    pub name: BoxName,
    /// Box size (`small`, `medium`, `large`, `xlarge`); `small` when unset.
    pub class: Option<String>,
    /// Random suffix that keeps the Box name unique.
    pub nonce: String,
}

/// A Box the Driver has handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Working directory.
    pub workdir: BoxPath,
}

/// Result of releasing a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    /// The Box was deleted.
    Deleted,
    /// The Box did not exist.
    AlreadyGone,
}

/// One Box as the Driver lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxListing {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Provider, when known.
    pub provider: Option<ProviderId>,
    /// Whether the Box sleeps.
    pub asleep: bool,
}

/// A command to run inside a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment for the command.
    pub env: Vec<(String, String)>,
    /// Directory to run in, made when missing; the Box's login directory when unset.
    pub cwd: Option<BoxPath>,
    /// Where to stream output; captured when unset.
    pub output: Option<Output>,
}

/// How a command inside a Box ended; a non-zero code is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecOutcome {
    /// The command finished with this exit code.
    Exited {
        /// Exit code of the command.
        code: i32,
    },
    /// The command was stopped by an interrupt or signal before it finished.
    Interrupted,
}

/// What `Driver::sleep` did for a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SleepOutcome {
    /// The Driver suspended the Box.
    Slept,
    /// The provider sleeps the Box on its own; nothing was called.
    Native,
    /// The Box stays up; the reason is worded for people.
    CannotSleep(String),
}

/// The only error shape Driver adapters return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// No such Box.
    NotFound,
    /// Slot limit reached.
    Capacity {
        /// Slot limit, when known.
        limit: Option<u32>,
        /// Who holds the slots, when known.
        holder: Option<String>,
    },
    /// Provider quota hit.
    Quota(String),
    /// Credentials rejected.
    Auth(String),
    /// The operation outlived its allowance.
    Timeout(Duration),
    /// Provider unreachable.
    Unavailable(String),
    /// Provider cannot do this.
    Unsupported(&'static str),
    /// A reverse tunnel stopped or stalled before it was ready.
    Tunnel {
        /// What went wrong.
        failure: TunnelFailure,
        /// Last lines the tunnel process wrote to stderr.
        stderr_tail: String,
    },
    /// The ssh connection to the Box dropped, so whether the remote command ran is unknown.
    Transport {
        /// Last lines crabbox or ssh wrote to stderr, as one line.
        stderr_tail: String,
    },
    /// Any other failure, summarised by the adapter.
    Failed {
        /// Moor-worded summary.
        summary: String,
    },
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "box not found"),
            Self::Capacity { limit, holder } => fmt_capacity(f, *limit, holder.as_deref()),
            Self::Quota(m) => write!(f, "provider quota reached: {m}"),
            Self::Auth(m) => write!(f, "provider authentication failed: {m}"),
            Self::Timeout(d) => write!(f, "box operation timed out after {}s", d.as_secs()),
            Self::Unavailable(m) => write!(f, "box provider unavailable: {m}"),
            Self::Unsupported(what) => write!(f, "not supported by this provider: {what}"),
            Self::Tunnel {
                failure,
                stderr_tail,
            } => fmt_tunnel(f, *failure, stderr_tail),
            Self::Transport { stderr_tail } if stderr_tail.is_empty() => {
                f.write_str("moor lost the ssh connection to the Box")
            }
            Self::Transport { stderr_tail } => {
                write!(f, "moor lost the ssh connection to the Box: {stderr_tail}")
            }
            Self::Failed { summary } => f.write_str(summary),
        }
    }
}

/// Writes the capacity message, naming limit and holder when known.
fn fmt_capacity(
    f: &mut fmt::Formatter<'_>,
    limit: Option<u32>,
    holder: Option<&str>,
) -> fmt::Result {
    write!(f, "box capacity reached")?;
    if let Some(n) = limit {
        write!(f, " (limit {n})")?;
    }
    if let Some(h) = holder {
        write!(f, ", held by {h}")?;
    }
    Ok(())
}

impl std::error::Error for DriverError {}

/// Boxes, behind whichever provider hub is wired.
pub trait Driver: Send + Sync {
    /// Reports what `provider` can do.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider is unknown or unreachable.
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError>;

    /// Gets a new Box for a Box row.
    ///
    /// # Errors
    /// Returns [`DriverError::Capacity`], `Quota`, `Auth` and the like.
    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError>;

    /// The handle `acquire` returns for `req`; pure, so a failed acquire can
    /// still name the half-made Box for release.
    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle;

    /// Releases a Box; an already-gone Box is not an error.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the release fails.
    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError>;

    /// How a person releases the Box by hand after [`Driver::release`] failed, worded for
    /// people; none when the Driver has nothing to add.
    fn manual_release(&self, handle: &BoxHandle) -> Option<String> {
        let _ = handle;
        None
    }

    /// Lists the Boxes the Driver knows under one provider. Each listing's
    /// handle equals the handle `acquire` returned for that Box.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider cannot be queried.
    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError>;

    /// Returns the Box's working directory.
    ///
    /// # Errors
    /// Returns [`DriverError::NotFound`] when the Box is gone, and
    /// [`DriverError::Failed`] when the provider cannot answer the lookup.
    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError>;

    /// Runs one command in the Box and reports its exit code.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the command cannot be run at all.
    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError>;

    /// Copies a file or directory out of the Box to the host path `to`, or into `to` when it is
    /// a directory. A failed copy leaves nothing at the destination.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the copy fails.
    fn copy_out(&self, handle: &BoxHandle, from: &BoxPath, to: &Path) -> Result<(), DriverError>;

    /// Streams the regular files `names` from directory `dir` in the Box in one round trip,
    /// calling `sink` once per file received, in stream order. `Some(reader)` yields exactly that
    /// file's bytes and need not be read to the end; `None` means the entry was not a regular file.
    /// A name never handed to `sink` was not received.
    ///
    /// # Errors
    /// [`DriverError::Unsupported`] when the provider or the Box cannot stream files; nothing was
    /// handed to `sink` then, so the caller copies one file at a time. [`DriverError::Timeout`] when
    /// the stream hangs, and another [`DriverError`] when it breaks; files already handed to `sink`
    /// stay valid.
    fn stream_files(
        &self,
        handle: &BoxHandle,
        dir: &BoxPath,
        names: &[String],
        deadline: Deadline,
        sink: &mut dyn FnMut(&str, Option<&mut dyn std::io::Read>),
    ) -> Result<(), DriverError> {
        let _ = (handle, dir, names, deadline, sink);
        Err(DriverError::Unsupported("streaming files"))
    }

    /// Reads one file in the Box without queueing behind a running command.
    /// `Ok(None)` means the file does not exist.
    ///
    /// # Errors
    /// Returns [`DriverError::Timeout`] when the read hangs, and another
    /// [`DriverError`] when the Box cannot be reached or the file cannot be read.
    fn peek_file(&self, handle: &BoxHandle, path: &BoxPath) -> Result<Option<String>, DriverError>;

    /// Writes `bytes` to `path` in the Box, creating missing parent directories,
    /// and leaves the file at `mode`. The content travels on stdin, never in argv.
    ///
    /// # Errors
    /// Returns [`DriverError::NotFound`] when the Box is gone,
    /// [`DriverError::Timeout`] when the write hangs, and another [`DriverError`]
    /// when the file cannot be written.
    fn put_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError>;

    /// The `ssh -t <target>` argv that opens a terminal on the Box, for a pane to run.
    /// Takes no lease lock.
    ///
    /// # Errors
    /// Returns [`DriverError::NotFound`] when the Box is gone, and another
    /// [`DriverError`] when the target cannot be looked up.
    fn connect_argv(&self, handle: &BoxHandle) -> Result<Vec<String>, DriverError>;

    /// Exposes the host Unix socket `socket` inside the Box on loopback `box_port` until the
    /// returned handle drops. It opens `socket` per Box connection, so it reaches a listener
    /// bound again at the same path.
    ///
    /// # Errors
    /// Returns [`DriverError::Unsupported`] when the provider has no reverse
    /// tunnel, [`DriverError::Tunnel`] when the tunnel exits or stalls before it
    /// is ready, and another [`DriverError`] as for the other calls.
    fn reverse_tunnel(
        &self,
        handle: &BoxHandle,
        box_port: u16,
        socket: &Path,
    ) -> Result<Box<dyn TunnelHandle>, DriverError>;

    /// Runs `steps` in order in one round trip where the provider can. `connect` is what
    /// `connect_argv` returned for `handle`; a driver may reach the Box through it rather than
    /// looking the target up again. The batch stops after a step that errs or a `Run` that does
    /// not exit 0, so the result holds one entry per step run, the last being the one that stopped it.
    fn batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        steps: &[BoxStep],
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        let _ = connect;
        run_each(self, handle, steps, deadline)
    }

    /// Puts the Box to sleep when its provider lets the Driver do so.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider cannot be asked or the suspend fails.
    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError>;

    /// Wakes a sleeping Box.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the resume fails.
    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError>;
}

#[cfg(test)]
mod tests;
