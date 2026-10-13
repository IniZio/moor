//! Driver seam: how moor gets, inspects and releases Boxes.
//!
//! Only the provisioning methods, exec, copy-out and file peek exist so far; sleep,
//! tunnel and connect are not added yet.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::domain::run::SyncMode;
use crate::name::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};
use crate::seam::proc::{Deadline, Output};

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

/// How far a provider can restrict Box egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressSupport {
    /// Always enforced.
    Enforced,
    /// Enforced when requested.
    OptIn,
    /// Not available.
    None,
}

/// Facts about a provider, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCaps {
    /// Sleep style.
    pub sleep: SleepKind,
    /// Whether a sleeping Box can wake.
    pub wake: bool,
    /// Egress restriction support.
    pub egress: EgressSupport,
    /// Box-to-host tunnel support.
    pub reverse_tunnel: bool,
    /// Whether the Box reports a working directory.
    pub has_workdir: bool,
}

/// The egress policy a new Box should get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPlan {
    /// No restriction.
    Open,
    /// Only these hosts.
    Allow(Vec<String>),
}

/// Whether the provider applied the requested egress plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressApplied {
    /// The plan is in force.
    Enforced,
    /// The provider ignored the plan.
    NotEnforced,
}

/// A request for a new Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireReq {
    /// Provider to ask.
    pub provider: ProviderId,
    /// Source for the per-Task Box name the adapter derives.
    pub task: TaskId,
    /// Task name the adapter embeds in the Box name.
    pub name: TaskName,
    /// Requested egress.
    pub egress: EgressPlan,
    /// Box size (`small`, `medium`, `large`, `xlarge`); `small` when unset.
    pub class: Option<String>,
    /// Random suffix that keeps the Box name unique.
    pub nonce: String,
    /// Host directory to run the warmup in.
    pub worktree: Option<PathBuf>,
}

/// A Box the Driver has handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Working directory.
    pub workdir: BoxPath,
    /// Egress outcome.
    pub egress: EgressApplied,
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
    /// Directory to run in; the Box workdir when unset.
    pub cwd: Option<BoxPath>,
    /// Whether to sync the worktree before running.
    pub sync: SyncMode,
    /// Where to stream output; captured when unset.
    pub output: Option<Output>,
    /// Host directory (the Task worktree) the driver process runs from.
    pub host_cwd: Option<PathBuf>,
    /// Box size (`small`, `medium`, `large`, `xlarge`); none is passed when unset.
    pub class: Option<String>,
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

    /// Gets a new Box for a Task.
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

    /// Lists the Boxes the Driver knows under one provider. Each listing's
    /// handle equals the handle `acquire` returned for that Box.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider cannot be queried.
    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError>;

    /// Returns the Box's working directory.
    ///
    /// # Errors
    /// Returns [`DriverError::NotFound`] when the Box is gone.
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

    /// Copies a file or directory out of the Box to the host. Never syncs the
    /// worktree. `host_cwd` is the worktree the driver process runs from.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the copy fails.
    fn copy_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        host_cwd: &Path,
    ) -> Result<(), DriverError>;

    /// Reads one file in the Box without queueing behind a running command.
    /// `Ok(None)` means the file does not exist. `host_cwd` is the worktree the
    /// driver process runs from.
    ///
    /// # Errors
    /// Returns [`DriverError::Timeout`] when the read hangs, and another
    /// [`DriverError`] when the Box cannot be reached or the file cannot be read.
    fn peek_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        host_cwd: &Path,
    ) -> Result<Option<String>, DriverError>;
}

#[cfg(test)]
mod tests;
